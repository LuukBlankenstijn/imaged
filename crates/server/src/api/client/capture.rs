use std::sync::Arc;

use crate::{
    api::client::{AgentMac, get_next_task},
    domain::{
        image::ImagePartition,
        task::{Task, TaskType},
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

async fn get_capture_task_and_verify(
    state: Arc<HandlerState>,
    mac: &str,
    image_id: i64,
) -> Result<Task> {
    let task = get_next_task(state, mac).await?;
    if task.image_id != Some(image_id) || task.task_type != TaskType::Capture {
        Err(AppError::InvalidArgument(format!(
            "No capture task for image {image_id} found"
        )))
    } else {
        Ok(task)
    }
}
