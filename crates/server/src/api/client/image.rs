use std::sync::Arc;

use crate::{
    api::client::AgentMac,
    domain::{
        image::ImagePartition,
        task::{Task, TaskState, TaskType},
    },
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
use serde::Deserialize;

use super::HandlerState;

pub async fn upload_partition_data(
    State(state): State<Arc<HandlerState>>,
    Path((image_id, partition_number)): Path<(i64, i64)>,
    AgentMac(mac): AgentMac,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse> {
    // verify this actually needs uploading
    let _ = get_capture_task_and_verify(state.clone(), &mac, image_id).await?;
    let fstype = headers
        .get("X-Fstype")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::InvalidArgument("missing X-Fstype".into()))?
        .to_string();
    let size: u64 = headers
        .get("X-Partition-Size")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or(AppError::InvalidArgument("missing X-Partition-Size".into()))?;
    let stream = body.into_data_stream().map_err(std::io::Error::other);

    let (path, sha) = state
        .image_service
        .save_partition_data(image_id, partition_number, stream)
        .await?;

    let partition = state
        .image_repo
        .save_partition(
            image_id,
            ImagePartition::new(-1, partition_number, fstype, size, path, sha),
        )
        .await?;

    Ok((StatusCode::CREATED, Json(partition)))
}

pub async fn download_partition_data(
    State(state): State<Arc<HandlerState>>,
    Path((image_id, partition_number)): Path<(i64, i64)>,
) -> Result<impl IntoResponse> {
    let stream = state
        .image_service
        .read_partition_data(image_id, partition_number)
        .await?;

    Ok(Body::from_stream(stream))
}

pub async fn upload_partition_table(
    State(state): State<Arc<HandlerState>>,
    Path(image_id): Path<i64>,
    AgentMac(mac): AgentMac,
    body: Bytes,
) -> Result<impl IntoResponse> {
    let task = get_capture_task_and_verify(state.clone(), &mac, image_id).await?;

    // remove old data
    state.image_service.clear_image_data(image_id).await?;
    // remove old partition records and marks image as capturing
    state.image_repo.start_capture(image_id).await?;
    // save partition table
    state
        .image_service
        .save_partition_table(image_id, &body)
        .await?;
    // update task state
    state.task_repo.start(task.id).await?;
    Ok(StatusCode::CREATED)
}

pub async fn mark_finished(
    State(state): State<Arc<HandlerState>>,
    Path(image_id): Path<i64>,
    AgentMac(mac): AgentMac,
) -> Result<impl IntoResponse> {
    let task = get_capture_task_and_verify(state.clone(), &mac, image_id).await?;
    if task.state == TaskState::Pending {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ));
    }

    state.image_repo.mark_finished(image_id).await?;
    state.task_repo.finish(task.id).await?;

    Ok(())
}

#[derive(Deserialize)]
pub struct MarkErrorPayload {
    error: String,
}

pub async fn mark_failed(
    State(state): State<Arc<HandlerState>>,
    Path(image_id): Path<i64>,
    AgentMac(mac): AgentMac,
    Json(body): Json<MarkErrorPayload>,
) -> Result<impl IntoResponse> {
    let task = get_capture_task_and_verify(state.clone(), &mac, image_id).await?;
    if task.state == TaskState::Pending {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ));
    }

    state.image_repo.mark_faulted(image_id, &body.error).await?;
    state.task_repo.mark_failed(task.id, &body.error).await?;
    Ok(())
}

async fn get_capture_task_and_verify(
    state: Arc<HandlerState>,
    mac: &str,
    image_id: i64,
) -> Result<Task> {
    let host = state.host_repo.get_by_mac(mac).await?;
    let task =
        state.task_repo.get_next(host.id).await?.ok_or_else(|| {
            AppError::InvalidArgument(format!("No active task found for host {mac}"))
        })?;
    if task.image_id != Some(image_id) || task.task_type != TaskType::Capture {
        Err(AppError::InvalidArgument(format!(
            "No capture task for image {image_id} found"
        )))
    } else {
        Ok(task)
    }
}
