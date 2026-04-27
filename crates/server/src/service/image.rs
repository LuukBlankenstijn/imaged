use crate::error::{AppError, Result};
use bytes::Bytes;
use derive_more::Constructor;
use futures::Stream;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

#[derive(Constructor, Debug)]
pub struct ImageService {
    images_path: String,
}

impl ImageService {
    pub async fn clear_image_data(&self, image_id: i64) -> Result<()> {
        let relative_dir = format!("img-{image_id}");
        let dir = format!("{}/{}", self.images_path, relative_dir);
        if tokio::fs::try_exists(&dir).await.unwrap_or(false) {
            tokio::fs::remove_dir_all(&dir).await.map_err(|e| {
                tracing::error!("failed to clean image dir: {e}");
                AppError::Internal(e.to_string())
            })?;
        }

        Ok(())
    }
    pub async fn save_partition_table(&self, image_id: i64, data: &[u8]) -> Result<String> {
        let relative_dir = format!("img-{image_id}");
        let dir = format!("{}/{}", self.images_path, relative_dir);
        let relative_filepath = format!("{relative_dir}/parttable.bin");
        let path = format!("{}/{}", self.images_path, relative_filepath);

        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            tracing::error!("failed to create image dir: {e}");
            AppError::Internal(e.to_string())
        })?;

        tokio::fs::write(&path, data).await.map_err(|e| {
            tracing::error!("failed to write parttable: {e}");
            AppError::Internal(e.to_string())
        })?;

        Ok(relative_filepath)
    }

    pub async fn read_partition_table(&self, image_id: i64) -> Result<Vec<u8>> {
        let path = format!("{}/img-{}/parttable.bin", self.images_path, image_id);
        let data = tokio::fs::read(&path).await.map_err(|e| {
            tracing::error!("failed to read partition table file {path}: {e}");
            AppError::Internal(e.to_string())
        })?;
        Ok(data)
    }

    pub async fn save_partition_data<S>(
        &self,
        image_id: i64,
        partition_number: i64,
        mut data_stream: S,
    ) -> Result<(String, String)>
    where
        S: Stream<Item = std::result::Result<Bytes, std::io::Error>> + Unpin + Send,
    {
        let relative_filepath = format!("img-{}/p-{}.pcl", image_id, partition_number);
        let file_path = format!("{}/{}", self.images_path, relative_filepath);
        let mut file = tokio::fs::File::create(&file_path).await.map_err(|e| {
            tracing::error!("failed to create partition file {file_path}: {e}");
            AppError::Internal(e.to_string())
        })?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = data_stream.next().await {
            let chunk = chunk.map_err(|e| {
                tracing::error!(
                    "error reading stream for image {image_id} partition {partition_number}: {e}"
                );
                AppError::InvalidArgument(format!("error reading stream: {e}"))
            })?;
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| {
                tracing::error!("failed to write chunk to {file_path}: {e}");
                AppError::Internal("failed to write stream chunk to file".to_string())
            })?;
        }
        let sha = hex::encode(hasher.finalize());
        Ok((relative_filepath, sha))
    }

    pub async fn read_partition_data(
        &self,
        image_id: i64,
        partition_number: i64,
    ) -> Result<impl Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send + use<>> {
        let relative_filepath = format!("img-{}/p-{}.pcl", image_id, partition_number);
        let file_path = format!("{}/{}", self.images_path, relative_filepath);
        let file = tokio::fs::File::open(&file_path).await.map_err(|e| {
            tracing::error!("failed to open partition file {file_path}: {e}");
            AppError::Internal(e.to_string())
        })?;
        Ok(tokio_util::io::ReaderStream::new(file))
    }
}
