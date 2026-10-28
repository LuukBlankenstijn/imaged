use chrono::{DateTime, Utc};
use imaged_model::error::Result;
use imaged_model::image::{ImageKind, ImagePartition, ImageStatus};

#[derive(Debug)]
pub struct Image {
    pub id: i64,
    pub name: String,
    pub kind: ImageKind,
    pub captured_at: Option<DateTime<Utc>>,
    pub status: ImageStatus,
    pub error: Option<String>,
    pub partitions: Vec<ImagePartition>,
    pub root_partition: Option<i64>,
    pub sector_size: u32,
}

#[derive(Debug)]
pub struct NewImagePartition {
    pub partition_number: i64,
    pub fstype: String,
    pub size_bytes: u64,
    pub fs_uuid: Option<String>,
    pub fs_label: Option<String>,
}

#[async_trait::async_trait]
pub trait ImageRepository: Send + Sync {
    async fn get_status(&self, id: i64) -> Result<ImageStatus>;
    async fn get(&self, id: i64) -> Result<Image>;
    async fn create_image(&self, name: String, kind: ImageKind) -> Result<Image>;
    async fn update_name(&self, id: i64, new_name: String) -> Result<Image>;
    async fn set_root_partition(&self, id: i64, partition_number: Option<i64>) -> Result<Image>;
    async fn get_all(&self) -> Result<Vec<Image>>;
    async fn save_partition(
        &self,
        image_id: i64,
        partition_number: i64,
        fstype: &str,
        size_bytes: i64,
        fs_uuid: Option<&str>,
        fs_label: Option<&str>,
    ) -> Result<ImagePartition>;
    async fn delete_image(&self, id: i64) -> Result;
    // clears all old data and marks the images as capturing
    async fn start_capture(&self, id: i64, sector_size: u32) -> Result;
    // marks the image as finished
    async fn mark_finished(&self, id: i64) -> Result;
    // marks the image as faulted
    async fn mark_faulted(&self, id: i64, error: &str) -> Result;
    async fn start_import(&self, name: String, kind: ImageKind) -> Result<Image>;
    async fn finish_import(
        &self,
        id: i64,
        captured_at: Option<DateTime<Utc>>,
        partitions: &[NewImagePartition],
        root_partition: Option<i64>,
        sector_size: u32,
    ) -> Result<Image>;
    async fn fault_unfinished_imports(&self, error: &str) -> Result;
    // get partitions
    async fn get_partitions(&self, id: i64) -> Result<Vec<ImagePartition>>;
}
