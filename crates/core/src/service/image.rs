use crate::error::{AppError, Result};
use bytes::Bytes;
use derive_more::Constructor;
use futures::Stream;
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

    pub fn get_partition_table_path(&self, image_id: i64) -> String {
        format!("{}/img-{}/parttable.bin", self.images_path, image_id)
    }

    pub fn get_partition_path(&self, image_id: i64, partition_number: i64) -> String {
        format!(
            "{}/img-{}/p-{}.pcl",
            self.images_path, image_id, partition_number
        )
    }

    pub async fn read_partition_table(&self, image_id: i64) -> Result<Vec<u8>> {
        let path = self.get_partition_table_path(image_id);
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
    ) -> Result
    where
        S: Stream<Item = std::result::Result<Bytes, AppError>> + Unpin + Send,
    {
        let file_path = self.get_partition_path(image_id, partition_number);
        let mut file = tokio::fs::File::create(&file_path).await.map_err(|e| {
            tracing::error!("failed to create partition file {file_path}: {e}");
            AppError::Internal(e.to_string())
        })?;
        while let Some(chunk) = data_stream.next().await {
            let chunk = chunk.map_err(|e| {
                tracing::error!(
                    "error reading stream for image {image_id} partition {partition_number}: {e}"
                );
                AppError::InvalidArgument(format!("error reading stream: {e}"))
            })?;
            file.write_all(&chunk).await.map_err(|e| {
                tracing::error!("failed to write chunk to {file_path}: {e}");
                AppError::Internal("failed to write stream chunk to file".to_string())
            })?;
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DB_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn service() -> (ImageService, TestDir) {
        let id = DB_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("imaged-imgsvc-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(&dir).unwrap();
        let svc = ImageService::new(dir.to_str().unwrap().to_string());
        (svc, TestDir(dir))
    }

    #[test]
    fn get_partition_table_path_pins_the_exact_on_disk_layout() {
        let svc = ImageService::new("/base".to_string());
        assert_eq!(svc.get_partition_table_path(7), "/base/img-7/parttable.bin");
    }

    #[test]
    fn get_partition_path_pins_the_exact_pcl_layout_including_extension() {
        let svc = ImageService::new("/base".to_string());
        assert_eq!(svc.get_partition_path(7, 2), "/base/img-7/p-2.pcl");
    }

    #[tokio::test]
    async fn save_partition_table_creates_the_image_dir_when_absent_and_round_trips() {
        let (svc, _d) = service();
        let rel = svc.save_partition_table(3, b"hello table").await.unwrap();
        assert_eq!(rel, "img-3/parttable.bin");
        let back = svc.read_partition_table(3).await.unwrap();
        assert_eq!(back, b"hello table");
    }

    #[tokio::test]
    async fn save_partition_table_overwrites_rather_than_appends() {
        let (svc, _d) = service();
        svc.save_partition_table(3, b"first version longer").await.unwrap();
        svc.save_partition_table(3, b"second").await.unwrap();
        let back = svc.read_partition_table(3).await.unwrap();
        assert_eq!(back, b"second");
    }

    #[tokio::test]
    async fn read_partition_table_for_a_missing_image_returns_an_internal_error_and_does_not_panic() {
        let (svc, _d) = service();
        let err = svc.read_partition_table(999).await.unwrap_err();
        assert!(matches!(err, AppError::Internal(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn save_partition_data_round_trips_the_exact_concatenation_of_a_multi_chunk_stream() {
        let (svc, _d) = service();
        svc.save_partition_table(4, b"table").await.unwrap();
        let chunks = vec![
            Ok(Bytes::from_static(b"aaaa")),
            Ok(Bytes::from_static(b"bbbbbb")),
            Ok(Bytes::from_static(b"cc")),
        ];
        svc.save_partition_data(4, 1, futures::stream::iter(chunks))
            .await
            .unwrap();
        let written = tokio::fs::read(svc.get_partition_path(4, 1)).await.unwrap();
        assert_eq!(written, b"aaaabbbbbbcc");
    }

    #[tokio::test]
    async fn save_partition_data_leaves_a_truncated_file_on_disk_when_the_stream_errors_midway() {
        let (svc, _d) = service();
        svc.save_partition_table(5, b"table").await.unwrap();
        let chunks: Vec<std::result::Result<Bytes, AppError>> = vec![
            Ok(Bytes::from_static(b"good1")),
            Ok(Bytes::from_static(b"good2")),
            Err(AppError::Internal("network drop".to_string())),
        ];
        let res = svc
            .save_partition_data(5, 1, futures::stream::iter(chunks))
            .await;
        assert!(res.is_err());
        let partial = tokio::fs::read(svc.get_partition_path(5, 1)).await.unwrap();
        assert_eq!(partial, b"good1good2");
    }

    #[tokio::test]
    async fn read_partition_data_streams_back_exactly_the_bytes_written() {
        let (svc, _d) = service();
        svc.save_partition_table(6, b"table").await.unwrap();
        let chunks = vec![
            Ok(Bytes::from_static(b"xxxx")),
            Ok(Bytes::from_static(b"yyyy")),
        ];
        svc.save_partition_data(6, 2, futures::stream::iter(chunks))
            .await
            .unwrap();
        let mut stream = svc.read_partition_data(6, 2).await.unwrap();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(out, b"xxxxyyyy");
    }

    #[tokio::test]
    async fn read_partition_data_for_a_missing_file_surfaces_an_error_rather_than_an_empty_stream() {
        let (svc, _d) = service();
        let err = svc.read_partition_data(6, 99).await.err();
        assert!(matches!(err, Some(AppError::Internal(_))), "got {err:?}");
    }

    #[tokio::test]
    async fn deleting_the_partition_file_after_the_stream_is_created_does_not_truncate_the_already_open_read() {
        let (svc, _d) = service();
        svc.save_partition_table(8, b"table").await.unwrap();
        let chunks = vec![Ok(Bytes::from_static(b"payload-bytes"))];
        svc.save_partition_data(8, 1, futures::stream::iter(chunks))
            .await
            .unwrap();
        let mut stream = svc.read_partition_data(8, 1).await.unwrap();
        tokio::fs::remove_file(svc.get_partition_path(8, 1)).await.unwrap();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(out, b"payload-bytes");
    }

    #[tokio::test]
    async fn clear_image_data_removes_the_image_dir() {
        let (svc, _d) = service();
        svc.save_partition_table(1, b"table").await.unwrap();
        assert!(tokio::fs::try_exists(format!("{}/img-1", svc.images_path)).await.unwrap());
        svc.clear_image_data(1).await.unwrap();
        assert!(!tokio::fs::try_exists(format!("{}/img-1", svc.images_path)).await.unwrap());
    }

    #[tokio::test]
    async fn clear_image_data_is_a_silent_no_op_when_the_dir_is_absent() {
        let (svc, _d) = service();
        svc.clear_image_data(12345).await.unwrap();
    }

    #[tokio::test]
    async fn clear_image_data_does_not_touch_sibling_img_dirs_with_a_shared_prefix() {
        let (svc, _d) = service();
        svc.save_partition_table(1, b"one").await.unwrap();
        svc.save_partition_table(10, b"ten").await.unwrap();
        svc.clear_image_data(1).await.unwrap();
        assert!(!tokio::fs::try_exists(format!("{}/img-1", svc.images_path)).await.unwrap());
        assert!(tokio::fs::try_exists(format!("{}/img-10", svc.images_path)).await.unwrap());
    }
}
