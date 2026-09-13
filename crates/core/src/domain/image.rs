use std::str::FromStr;

use derive_more::{Constructor, Display, FromStr, IsVariant};
use serde::Serialize;

use crate::error::{AppError, Result};
use chrono::{DateTime, Utc};

#[derive(Debug, Display, FromStr, IsVariant, PartialEq)]
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
}

#[async_trait::async_trait]
pub trait ImageRepository: Send + Sync {
    async fn get_status(&self, id: i64) -> Result<ImageStatus>;
    async fn create_image(&self, name: String) -> Result<Image>;

    async fn update_name(&self, id: i64, new_name: String) -> Result<Image>;

    async fn get_all(&self) -> Result<Vec<Image>>;

    async fn save_partition(
        &self,
        image_id: i64,
        partition_number: i64,
        fstype: &str,
        size_bytes: i64,
    ) -> Result<ImagePartition>;

    async fn delete_image(&self, id: i64) -> Result;

    // clears all old data and marks the images as capturing
    async fn start_capture(&self, id: i64) -> Result;

    // marks the image as finished
    async fn mark_finished(&self, id: i64) -> Result;

    // marks the image as faulted
    async fn mark_faulted(&self, id: i64, error: &str) -> Result;

    // get partitions
    async fn get_partitions(&self, id: i64) -> Result<Vec<ImagePartition>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_status_round_trips_through_display_and_from_string_for_every_variant() {
        for s in [
            ImageStatus::Empty,
            ImageStatus::Capturing,
            ImageStatus::Ready,
            ImageStatus::Faulted,
        ] {
            assert_eq!(ImageStatus::from_string(s.to_string()).unwrap(), s);
        }
    }

    #[test]
    fn image_status_display_is_lowercase() {
        assert_eq!(ImageStatus::Empty.to_string(), "empty");
        assert_eq!(ImageStatus::Capturing.to_string(), "capturing");
        assert_eq!(ImageStatus::Ready.to_string(), "ready");
        assert_eq!(ImageStatus::Faulted.to_string(), "faulted");
    }

    #[test]
    fn every_status_the_repository_writes_parses_back_so_the_repository_expect_cannot_panic() {
        for stored in ["empty", "capturing", "ready", "faulted"] {
            assert!(
                ImageStatus::from_string(stored.to_string()).is_ok(),
                "status {stored:?} written by repository must parse"
            );
        }
    }

    #[test]
    fn image_status_from_string_is_case_insensitive_as_derive_more_implements_it() {
        assert_eq!(
            ImageStatus::from_string("Empty".to_string()).unwrap(),
            ImageStatus::Empty
        );
        assert_eq!(
            ImageStatus::from_string("READY".to_string()).unwrap(),
            ImageStatus::Ready
        );
        assert_eq!(
            ImageStatus::from_string("cApTuRiNg".to_string()).unwrap(),
            ImageStatus::Capturing
        );
    }

    #[test]
    fn image_status_from_string_rejects_unknown_with_internal_conversion_error() {
        let err = ImageStatus::from_string("bogus".to_string()).unwrap_err();
        match err {
            AppError::Internal(msg) => assert_eq!(msg, "conversion error"),
            other => panic!("expected AppError::Internal, got {other:?}"),
        }
    }
}
