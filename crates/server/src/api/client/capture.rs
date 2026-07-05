use std::sync::Arc;

use crate::{
    api::client::{AgentMac, get_next_task},
    domain::task::{Task, TaskState, TaskType},
    error::{AppError, Result},
};
use axum::{
    Json,
    body::Body,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use futures::TryStreamExt;

use super::HandlerState;
pub async fn upload_partition_data(
    State(state): State<Arc<HandlerState>>,
    Path((task_id, partition_number)): Path<(i64, i64)>,
    AgentMac(mac): AgentMac,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse> {
    let (task, image_id) = get_capture_task_and_verify(state.clone(), &mac, task_id).await?;
    // Capture is single-host, so the aggregate equals that host's state.
    if task.aggregate_state() != TaskState::Running {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ));
    }
    let fstype = headers
        .get("X-Fstype")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::InvalidArgument("missing X-Fstype".into()))?
        .to_string();
    let size: i64 = headers
        .get("X-Partition-Size")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or(AppError::InvalidArgument("missing X-Partition-Size".into()))?;
    let stream = body.into_data_stream().map_err(std::io::Error::other);

    state
        .image_service
        .save_partition_data(image_id, partition_number, stream)
        .await?;

    let partition = state
        .image_repo
        .save_partition(image_id, partition_number, &fstype, size)
        .await?;

    Ok((StatusCode::CREATED, Json(partition)))
}

pub async fn upload_partition_table(
    State(state): State<Arc<HandlerState>>,
    Path(task_id): Path<i64>,
    AgentMac(mac): AgentMac,
    body: Bytes,
) -> Result<impl IntoResponse> {
    let (task, image_id) = get_capture_task_and_verify(state.clone(), &mac, task_id).await?;
    if task.aggregate_state() != TaskState::Pending {
        return Err(AppError::InvalidArgument(
            "Task has already stared".to_string(),
        ));
    }

    // remove old data
    state.image_service.clear_image_data(image_id).await?;
    // remove old partition records and marks image as capturing
    state.image_repo.start_capture(image_id).await?;
    // save partition table
    state
        .image_service
        .save_partition_table(image_id, &body)
        .await?;
    // mark this host's row running
    let host_id = state.host_repo.get_by_mac(&mac).await?.id;
    state.task_repo.start(task.id, host_id).await?;
    Ok(StatusCode::CREATED)
}

// verifies the task is the next task for the host and checks if the task is in the correct state
async fn get_capture_task_and_verify(
    state: Arc<HandlerState>,
    mac: &str,
    task_id: i64,
) -> Result<(Task, i64)> {
    let task = get_next_task(state, mac).await?;
    let Some(image_id) = task.image_id else {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} is not valid"
        )));
    };
    if task.id != task_id || task.task_type != TaskType::Capture {
        Err(AppError::InvalidArgument(format!(
            "No capture task for task {task_id} found"
        )))
    } else {
        Ok((task, image_id))
    }
}
