use std::sync::Arc;

use crate::{
    api::client::{AgentMac, get_next_task},
    domain::{
        image::ImageStatus,
        task::{Task, TaskType},
    },
    error::{AppError, Result},
};
use axum::{
    body::Body,
    extract::{Path, State},
    response::IntoResponse,
};

use super::HandlerState;

pub async fn download_partition_data(
    State(state): State<Arc<HandlerState>>,
    Path((task_id, partition_number)): Path<(i64, i64)>,
    AgentMac(mac): AgentMac,
) -> Result<impl IntoResponse> {
    let (_, image_id) = get_deploy_task_and_verify(state.clone(), &mac, task_id).await?;
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
    Path(task_id): Path<i64>,
    AgentMac(mac): AgentMac,
) -> Result<impl IntoResponse> {
    let (task, image_id) = get_deploy_task_and_verify(state.clone(), &mac, task_id).await?;
    if state.image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    state.task_repo.start(task.id).await?;
    let data = state.image_service.read_partition_table(image_id).await?;
    Ok(Body::from(data))
}

async fn get_deploy_task_and_verify(
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

    if task.id != task_id || task.task_type != TaskType::Deploy {
        Err(AppError::InvalidArgument(format!(
            "No deploy task for task {task_id} found"
        )))
    } else {
        Ok((task, image_id))
    }
}
