use std::sync::Arc;

use crate::{
    domain::image::ImagePartition,
    error::{AppError, Result},
};
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use futures::TryStreamExt;

use super::HandlerState;

pub async fn upload_partition_data(
    State(state): State<Arc<HandlerState>>,
    Path((image_id, partition_number)): Path<(i64, i64)>,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse> {
    let fstype = headers
        .get("X-Fstype")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::InvalidArgument("missing X-Fstype".into()))?
        .to_string();
    let size: u64 = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or(AppError::InvalidArgument("missing Content-Length".into()))?;
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
