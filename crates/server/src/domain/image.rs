use std::str::FromStr;

use derive_more::{Constructor, Display, FromStr, IsVariant};
use serde::Serialize;

use crate::error::{AppError, Result};
use chrono::{DateTime, Utc};

#[derive(Debug, Display, FromStr, IsVariant)]
#[display(rename_all = "lowercase")]
pub enum ImageStatus {
    Empty,
    Capturing,
    Ready,
    Faulted,
}

impl ImageStatus {
    pub fn from_string(value: String) -> Result<Self> {
        ImageStatus::from_str(&value).map_err(|e| {
            tracing::info!(err=%e, "failed to convert string to ImageStatus");
            AppError::Internal("conversion error".to_string())
        })
    }
}

#[derive(Debug, Constructor)]
pub struct Image {
    pub id: i64,
    pub name: String,
    pub captured_at: Option<DateTime<Utc>>,
    pub status: ImageStatus,
    pub error: Option<String>,
    pub partitions: Vec<ImagePartition>,
}

#[derive(Debug, Constructor, Serialize)]
pub struct ImagePartition {
    pub id: i64,
    pub partition_number: i64,
    pub fstype: String,
    pub size_bytes: u64,
    pub filepath: String,
    pub sha256: String,
}

#[async_trait::async_trait]
pub trait ImageRepository: Send + Sync {
    async fn create_image(&self, name: String) -> Result<Image>;

    async fn update_name(&self, id: i64, new_name: String) -> Result<Image>;

    async fn get_all(&self) -> Result<Vec<Image>>;

    async fn save_partition(
        &self,
        image_id: i64,
        partition: ImagePartition,
    ) -> Result<ImagePartition>;

    async fn delete_image(&self, id: i64) -> Result;
}
