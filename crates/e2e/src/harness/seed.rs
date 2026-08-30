use tokio::io::{AsyncReadExt, BufReader};

use imaged_core::domain::host::Host;
use imaged_core::domain::task::TaskType;

use super::TestServer;

pub async fn host(s: &TestServer, mac: &str, disk_size_bytes: u64) -> Host {
    s.container
        .host_repo
        .upsert_host(mac.to_string(), disk_size_bytes, None)
        .await
        .expect("seed host")
}

/// Create a Ready image whose partition-table and (zstd-compressed) partition
/// blobs are laid out exactly where `ImageService` reads them from. Each
/// partition is `(partition_number, fstype, uncompressed_bytes)`; the recorded
/// `size_bytes` is the uncompressed length.
pub async fn image_with_data(
    s: &TestServer,
    name: &str,
    parttable: &[u8],
    partitions: &[(i64, &str, &[u8])],
) -> i64 {
    let image = s
        .container
        .image_repo
        .create_image(name.to_string())
        .await
        .expect("create image");

    s.container
        .image_service
        .save_partition_table(image.id, parttable)
        .await
        .expect("save partition table");

    for &(number, fstype, plain) in partitions {
        let compressed = zstd(plain).await;
        let path = s.container.image_service.get_partition_path(image.id, number);
        tokio::fs::write(&path, &compressed)
            .await
            .expect("write partition data");
        s.container
            .image_repo
            .save_partition(image.id, number, fstype, plain.len() as i64)
            .await
            .expect("save partition record");
    }

    s.container
        .image_repo
        .mark_finished(image.id)
        .await
        .expect("mark image ready");

    image.id
}

pub async fn deploy_task(s: &TestServer, host_id: i64, image_id: i64) -> i64 {
    s.container
        .task_repo
        .create(TaskType::Deploy, vec![host_id], Some(image_id))
        .await
        .expect("create deploy task")
        .id
}

pub async fn capture_task(s: &TestServer, host_id: i64, image_id: i64) -> i64 {
    s.container
        .task_repo
        .create(TaskType::Capture, vec![host_id], Some(image_id))
        .await
        .expect("create capture task")
        .id
}

pub async fn reboot_task(s: &TestServer, host_id: i64) -> i64 {
    s.container
        .task_repo
        .create(TaskType::Reboot, vec![host_id], None)
        .await
        .expect("create reboot task")
        .id
}

/// Compress bytes the way the agent's capture path does before upload.
pub async fn zstd(plain: &[u8]) -> Vec<u8> {
    use async_compression::tokio::bufread::ZstdEncoder;
    let mut encoder = ZstdEncoder::new(BufReader::new(plain));
    let mut out = Vec::new();
    encoder.read_to_end(&mut out).await.expect("zstd compress");
    out
}

/// Decompress bytes the way the agent's deploy path does after download.
pub async fn unzstd(compressed: &[u8]) -> Vec<u8> {
    use async_compression::tokio::bufread::ZstdDecoder;
    let mut decoder = ZstdDecoder::new(BufReader::new(compressed));
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).await.expect("zstd decompress");
    out
}
