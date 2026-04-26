use std::fmt::Display;

use derive_more::Constructor;
use serde::Serialize;

use crate::error::Result;
use chrono::{DateTime, Utc};

#[derive(Debug)]
pub enum ImageStatus {
    Empty,
    Capturing,
    Ready,
    Faulted(String),
}

impl ImageStatus {
    pub fn into_parts(self) -> (String, Option<String>) {
        match self {
            ImageStatus::Empty => ("empty".to_string(), None),
            ImageStatus::Capturing => ("capturing".to_string(), None),
            ImageStatus::Ready => ("ready".to_string(), None),
            ImageStatus::Faulted(msg) => ("faulted".to_string(), Some(msg)),
        }
    }
}

impl From<String> for ImageStatus {
    fn from(value: String) -> Self {
        let val = value.to_lowercase();
        match val.as_str() {
            "empty" => ImageStatus::Empty,
            "capturing" => ImageStatus::Capturing,
            "ready" => ImageStatus::Ready,
            _ if val.starts_with("faulted(") && val.ends_with(')') => {
                let msg = &val[8..val.len() - 1];
                ImageStatus::Faulted(msg.to_string())
            }
            _ => ImageStatus::Faulted("invalid status".to_string()),
        }
    }
}

impl Display for ImageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let string = match self {
            ImageStatus::Empty => "empty".to_string(),
            ImageStatus::Capturing => "capturing".to_string(),
            ImageStatus::Ready => "ready".to_string(),
            ImageStatus::Faulted(msg) => format!("faulted({msg})"),
        };

        write!(f, "{}", string)
    }
}

#[derive(Debug, Constructor)]
pub struct Image {
    pub id: i64,
    pub name: String,
    pub captured_at: Option<DateTime<Utc>>,
    pub status: ImageStatus,
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
