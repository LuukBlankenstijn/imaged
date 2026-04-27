mod capture;
mod deploy;
mod sse;

use std::sync::Arc;

use axum::{
    Router,
    extract::{FromRequestParts, Query},
    http::request::Parts,
    routing::{get, post, put},
};
use serde::Deserialize;

use crate::{api::HandlerState, error::AppError};

pub fn router() -> Router<Arc<HandlerState>> {
    Router::new()
        .route(
            "/client/images/{image_id}/partitions/{partition_number}/data",
            put(capture::upload_partition_data).get(deploy::download_partition_data),
        )
        .route(
            "/client/images/{image_id}/parttable",
            put(capture::upload_partition_table).get(deploy::download_partition_table),
        )
        .route(
            "/client/capture/{image_id}/finished",
            post(capture::mark_finished),
        )
        .route(
            "/client/capture/{image_id}/failed",
            post(capture::mark_failed),
        )
        .route(
            "/client/deploy/{image_id}/finished",
            post(deploy::mark_finished),
        )
        .route(
            "/client/deploy/{image_id}/failed",
            post(deploy::mark_failed),
        )
        .route("/client/hosts/stream", get(sse::start_stream))
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
