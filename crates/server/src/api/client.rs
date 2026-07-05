mod capture;
mod deploy;
mod sse;
mod task;

use std::sync::Arc;

use axum::{
    Router,
    extract::{FromRequestParts, Query},
    http::request::Parts,
    routing::{get, post, put},
};
use serde::Deserialize;

use crate::{api::HandlerState, domain::task::Task, error::AppError};

pub fn router() -> Router<Arc<HandlerState>> {
    Router::new()
        .route(
            "/client/tasks/{task_id}/partitions/{partition_number}/data",
            put(capture::upload_partition_data).get(deploy::download_partition_data),
        )
        .route(
            "/client/tasks/{task_id}/parttable",
            put(capture::upload_partition_table).get(deploy::download_partition_table),
        )
        .route(
            "/client/tasks/{task_id}/finished",
            post(task::mark_finished),
        )
        .route("/client/tasks/{task_id}/faulted", post(task::mark_faulted))
        .route("/client/hosts/stream", get(sse::start_stream))
        .route("/client/hosts/disconnect", post(sse::disconnect))
}

pub struct AgentMac(pub String);

#[derive(Deserialize)]
struct MacQuery {
    mac: Option<String>,
}

impl<S> FromRequestParts<S> for AgentMac
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // try header first
        if let Some(mac) = parts
            .headers
            .get("X-Agent-Mac")
            .and_then(|v| v.to_str().ok())
        {
            return Ok(AgentMac(mac.to_string()));
        }

        // fall back to query
        let Query(q) = Query::<MacQuery>::from_request_parts(parts, _state)
            .await
            .map_err(|_| AppError::InvalidArgument("missing mac".into()))?;

        q.mac
            .map(AgentMac)
            .ok_or(AppError::InvalidArgument("missing mac".into()))
    }
}

async fn get_next_task(state: Arc<HandlerState>, mac: &str) -> crate::error::Result<Task> {
    let host = state.host_repo.get_by_mac(mac).await?;
    let task =
        state.task_repo.get_next(host.id).await?.ok_or_else(|| {
            AppError::InvalidArgument(format!("No active task found for host {mac}"))
        })?;
    Ok(task)
}
