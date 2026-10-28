use std::collections::BTreeMap;

use tokio::io::{AsyncReadExt, BufReader};

use imaged_core::domain::host::Host;
use imaged_core::domain::task::TaskType;
use imaged_model::image::ImageKind;

use super::TestServer;

pub async fn host(s: &TestServer, mac: &str, disk_size_bytes: u64) -> Host {
    let hosts = s.container.host_repo.clone();
    let mac = mac.to_string();
    s.on_server(async move { hosts.upsert_host(mac, Ok(disk_size_bytes), None).await })
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
    let mut compressed = Vec::with_capacity(partitions.len());
    for &(number, fstype, plain) in partitions {
        compressed.push((
            number,
            fstype.to_string(),
            plain.len() as i64,
            zstd(plain).await,
        ));
    }
    let container = s.container.clone();
    let name = name.to_string();
    let parttable = parttable.to_vec();
    s.on_server(async move {
        let image = container
            .image_repo
            .create_image(name, ImageKind::Disk)
            .await
            .expect("create image");
        container
            .image_service
            .save_partition_table(image.id, &parttable)
            .await
            .expect("save partition table");
        for (number, fstype, size_bytes, data) in compressed {
            let path = container.image_service.get_partition_path(image.id, number);
            tokio::fs::write(&path, &data)
                .await
                .expect("write partition data");
            container
                .image_repo
                .save_partition(image.id, number, &fstype, size_bytes, None, None)
                .await
                .expect("save partition record");
        }
        container
            .image_repo
            .mark_finished(image.id)
            .await
            .expect("mark image ready");
        image.id
    })
    .await
}

async fn task(s: &TestServer, task_type: TaskType, host_id: i64, image_id: Option<i64>) -> i64 {
    let tasks = s.container.task_repo.clone();
    s.on_server(async move { tasks.create(task_type, vec![host_id], image_id).await })
        .await
        .expect("create task")
        .id
}

pub async fn deploy_task(s: &TestServer, host_id: i64, image_id: i64) -> i64 {
    task(s, TaskType::Deploy, host_id, Some(image_id)).await
}

pub async fn capture_task(s: &TestServer, host_id: i64, image_id: i64) -> i64 {
    task(s, TaskType::Capture, host_id, Some(image_id)).await
}

pub async fn reboot_task(s: &TestServer, host_id: i64) -> i64 {
    task(s, TaskType::Reboot, host_id, None).await
}

pub async fn partition_image(
    s: &TestServer,
    name: &str,
    fstype: &str,
    plain: &[u8],
    fs_uuid: &str,
) -> i64 {
    let compressed = zstd(plain).await;
    let size_bytes = plain.len() as i64;
    let container = s.container.clone();
    let (name, fstype, fs_uuid) = (name.to_string(), fstype.to_string(), fs_uuid.to_string());
    s.on_server(async move {
        let image = container
            .image_repo
            .create_image(name, ImageKind::Partition)
            .await
            .expect("create image");
        tokio::fs::create_dir_all(container.image_service.image_dir(image.id))
            .await
            .expect("create image dir");
        let path = container.image_service.get_partition_path(image.id, 1);
        tokio::fs::write(&path, &compressed)
            .await
            .expect("write partition data");
        container
            .image_repo
            .save_partition(image.id, 1, &fstype, size_bytes, Some(&fs_uuid), None)
            .await
            .expect("save partition record");
        container
            .image_repo
            .mark_finished(image.id)
            .await
            .expect("mark image ready");
        image.id
    })
    .await
}

pub async fn partition_task(
    s: &TestServer,
    task_type: TaskType,
    host_id: i64,
    image_id: i64,
    partuuid: &str,
) -> i64 {
    let tasks = s.container.task_repo.clone();
    let partuuids = BTreeMap::from([(host_id, partuuid.to_string())]);
    s.on_server(async move {
        tasks
            .create_with_inputs(
                task_type,
                vec![host_id],
                Some(image_id),
                &BTreeMap::new(),
                &partuuids,
            )
            .await
    })
    .await
    .expect("create partition task")
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
    decoder
        .read_to_end(&mut out)
        .await
        .expect("zstd decompress");
    out
}
