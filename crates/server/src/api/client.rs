mod host;
mod image;

use std::sync::Arc;

use axum::{
    Router,
    extract::{FromRequestParts, Query},
    http::request::Parts,
    routing::{get, put},
};
use serde::Deserialize;

use crate::{api::HandlerState, error::AppError};

pub fn router() -> Router<Arc<HandlerState>> {
    Router::new()
        .route(
            "/client/images/{image_id}/partitions/{partition_number}/data",
            put(image::upload_partition_data).get(image::download_partition_data),
        )
        .route("/client/hosts/stream", get(host::start_stream))
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
