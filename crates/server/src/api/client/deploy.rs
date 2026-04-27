use std::sync::Arc;

use crate::{
    api::client::AgentMac,
    domain::{
        image::ImageStatus,
        task::{Task, TaskState, TaskType},
    },
    error::{AppError, Result},
};
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::Deserialize;

use super::HandlerState;

pub async fn download_partition_data(
    State(state): State<Arc<HandlerState>>,
    Path((image_id, partition_number)): Path<(i64, i64)>,
) -> Result<impl IntoResponse> {
    if state.image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    let stream = state
        .image_service
        .read_partition_data(image_id, partition_number)
        .await?;

    Ok(Body::from_stream(stream))
}

pub async fn download_partition_table(
    State(state): State<Arc<HandlerState>>,
    Path(image_id): Path<i64>,
    AgentMac(mac): AgentMac,
) -> Result<impl IntoResponse> {
    let task = get_deploy_task_and_verify(state.clone(), &mac, image_id).await?;
    if state.image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    state.task_repo.start(task.id).await?;
    let data = state.image_service.read_partition_table(image_id).await?;
    Ok(Body::from(data))
}

pub async fn mark_finished(
    State(state): State<Arc<HandlerState>>,
    Path(image_id): Path<i64>,
    AgentMac(mac): AgentMac,
) -> Result<impl IntoResponse> {
    let task = get_deploy_task_and_verify(state.clone(), &mac, image_id).await?;
    if task.state == TaskState::Pending {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ));
    }

    state.task_repo.mark_finished(task.id).await?;
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
    let task = get_deploy_task_and_verify(state.clone(), &mac, image_id).await?;
    if task.state == TaskState::Pending {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ));
    }

    state.task_repo.mark_failed(task.id, &body.error).await?;
    Ok(())
}

async fn get_deploy_task_and_verify(
    state: Arc<HandlerState>,
    mac: &str,
    image_id: i64,
) -> Result<Task> {
    let host = state.host_repo.get_by_mac(mac).await?;
    let task =
        state.task_repo.get_next(host.id).await?.ok_or_else(|| {
            AppError::InvalidArgument(format!("No active task found for host {mac}"))
        })?;
    if task.image_id != Some(image_id) || task.task_type != TaskType::Deploy {
        Err(AppError::InvalidArgument(format!(
            "No deploy task for image {image_id} found"
        )))
    } else {
        Ok(task)
    }
}
