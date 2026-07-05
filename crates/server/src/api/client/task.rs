use std::sync::Arc;

use super::AgentInfo;
use super::HandlerState;
use crate::api::client::get_next_task;
use crate::domain::task::TaskType;
use crate::error::AppError;
use crate::error::Result;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::Deserialize;

pub async fn mark_finished(
    State(state): State<Arc<HandlerState>>,
    Path(task_id): Path<i64>,
    AgentInfo((mac, _)): AgentInfo,
) -> Result<impl IntoResponse> {
    let task = get_next_task(state.clone(), &mac).await?;
    if task.id != task_id {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} is not the next task for host {mac}",
        )));
    }
    let host_id = state.host_repo.get_by_mac(&mac).await?.id;
    if task.task_type == TaskType::Capture
        && let Some(image_id) = task.image_id
    {
        state.image_repo.mark_finished(image_id).await?;
    }
    state.task_repo.mark_finished(task.id, host_id).await?;

    Ok(())
}

#[derive(Deserialize)]
pub struct MarkErrorPayload {
    error: String,
}

pub async fn mark_faulted(
    State(state): State<Arc<HandlerState>>,
    Path(task_id): Path<i64>,
    AgentInfo((mac, _)): AgentInfo,
    Json(body): Json<MarkErrorPayload>,
) -> Result<impl IntoResponse> {
    let task = get_next_task(state.clone(), &mac).await?;
    if task.id != task_id {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} is not the next task for host {mac}",
        )));
    }
    let host_id = state.host_repo.get_by_mac(&mac).await?.id;
    if task.task_type == TaskType::Capture
        && let Some(image_id) = task.image_id
    {
        state.image_repo.mark_faulted(image_id, &body.error).await?;
    }
    state
        .task_repo
        .mark_failed(task.id, host_id, &body.error)
        .await?;

    Ok(())
}
