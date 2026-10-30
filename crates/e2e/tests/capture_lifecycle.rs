use bytes::Bytes;
use futures::stream;
use reqwest::StatusCode;
use serde_json::json;

use imaged_e2e::harness::{self, seed};
use imaged_model::agent::{MAC_HEADER, path};
use imaged_model::image::{ImageKind, ImagePartition, ImageStatus};
use imaged_model::task::TaskState;

const DISK: u64 = 8 * 1024 * 1024 * 1024;

fn started_uri(task_id: i64) -> String {
    format!("{}?sector_size=512", path::started(task_id))
}

async fn start_capture(s: &harness::TestServer, task_id: i64, mac: &str) {
    let resp = s.agent_post_empty(&started_uri(task_id), mac).await;
    harness::expect_status(resp, StatusCode::OK).await;
}

async fn create_empty_image(s: &harness::TestServer, name: &str) -> i64 {
    let images = s.container.image_repo.clone();
    let name = name.to_string();
    s.on_server(async move { images.create_image(name, ImageKind::Disk).await })
        .await
        .expect("create empty image")
        .id
}

async fn image_status(s: &harness::TestServer, image_id: i64) -> ImageStatus {
    let images = s.container.image_repo.clone();
    s.on_server(async move { images.get_status(image_id).await })
        .await
        .expect("image status")
}

async fn captured_at_is_set(s: &harness::TestServer, image_id: i64) -> bool {
    image(s, image_id).await.captured_at.is_some()
}

async fn image_error(s: &harness::TestServer, image_id: i64) -> Option<String> {
    image(s, image_id).await.error
}

async fn image(s: &harness::TestServer, image_id: i64) -> imaged_core::domain::image::Image {
    let images = s.container.image_repo.clone();
    s.on_server(async move { images.get(image_id).await })
        .await
        .expect("image row")
}

async fn partitions(s: &harness::TestServer, image_id: i64) -> Vec<ImagePartition> {
    let images = s.container.image_repo.clone();
    s.on_server(async move { images.get_partitions(image_id).await })
        .await
        .expect("image partitions")
}

async fn aggregate_state(s: &harness::TestServer, task_id: i64) -> TaskState {
    let tasks = s.container.task_repo.clone();
    s.on_server(async move { tasks.get(task_id).await })
        .await
        .expect("task")
        .aggregate_state()
}

fn split(data: &[u8], chunk: usize) -> Vec<Bytes> {
    data.chunks(chunk).map(Bytes::copy_from_slice).collect()
}

async fn upload_partition(
    s: &harness::TestServer,
    task_id: i64,
    mac: &str,
    number: i64,
    fstype: &str,
    size: usize,
    body: Vec<Bytes>,
) -> reqwest::Response {
    let body = stream::iter(body.into_iter().map(Ok::<Bytes, std::io::Error>));
    s.agent_put_stream(
        &format!(
            "{}?filesystem={fstype}&size={size}",
            path::partition_data(task_id, number)
        ),
        mac,
        body,
    )
    .await
}

#[tokio::test]
async fn capture_happy_path_uploads_and_yields_a_usable_image() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    assert_eq!(image_status(s, image_id).await, ImageStatus::Empty);

    let task_id = seed::capture_task(s, host.id, image_id).await;

    let table: Vec<u8> = b"e2e-capture-partition-table".to_vec();
    start_capture(s, task_id, &mac).await;
    let resp = s
        .agent_put_bytes(&path::partition_table(task_id), &mac, &table)
        .await;
    harness::expect_status(resp, StatusCode::OK).await;
    assert_eq!(image_status(s, image_id).await, ImageStatus::Capturing);
    assert_eq!(aggregate_state(s, task_id).await, TaskState::Running);

    let plain1 = vec![0x11u8; 5000];
    let plain2 = vec![0x22u8; 3000];
    let comp1 = seed::zstd(&plain1).await;
    let comp2 = seed::zstd(&plain2).await;

    let resp = upload_partition(
        s,
        task_id,
        &mac,
        1,
        "ext4",
        plain1.len(),
        split(&comp1, 1024),
    )
    .await;
    harness::expect_status(resp, StatusCode::OK).await;

    let resp = upload_partition(s, task_id, &mac, 2, "xfs", plain2.len(), split(&comp2, 700)).await;
    harness::expect_status(resp, StatusCode::OK).await;

    let parts = partitions(s, image_id).await;
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].partition_number, 1);
    assert_eq!(parts[0].fstype, "ext4");
    assert_eq!(parts[0].size_bytes, plain1.len() as u64);
    assert_eq!(parts[1].partition_number, 2);
    assert_eq!(parts[1].fstype, "xfs");
    assert_eq!(parts[1].size_bytes, plain2.len() as u64);

    let on_disk1 = tokio::fs::read(s.container.image_service.get_partition_path(image_id, 1))
        .await
        .expect("read p1");
    assert_eq!(on_disk1, comp1);
    let on_disk2 = tokio::fs::read(s.container.image_service.get_partition_path(image_id, 2))
        .await
        .expect("read p2");
    assert_eq!(on_disk2, comp2);
    let on_disk_table =
        tokio::fs::read(s.container.image_service.get_partition_table_path(image_id))
            .await
            .expect("read parttable");
    assert_eq!(on_disk_table, table);

    let resp = s.agent_post_empty(&path::finished(task_id), &mac).await;
    harness::expect_status(resp, StatusCode::OK).await;
    assert_eq!(image_status(s, image_id).await, ImageStatus::Ready);
    assert!(captured_at_is_set(s, image_id).await);

    let other_mac = harness::unique_mac();
    let other = seed::host(s, &other_mac, DISK).await;
    let deploy_id = seed::deploy_task(s, other.id, image_id).await;

    let resp = s
        .agent_get(&path::partition_data(deploy_id, 1), &other_mac)
        .await;
    let resp = harness::expect_status(resp, StatusCode::OK).await;
    let downloaded = resp.bytes().await.expect("download p1");
    assert_eq!(downloaded.as_ref(), comp1.as_slice());
    assert_eq!(seed::unzstd(&downloaded).await, plain1);

    let resp = s
        .agent_get(&path::partition_table(deploy_id), &other_mac)
        .await;
    let resp = harness::expect_status(resp, StatusCode::OK).await;
    let table_back = resp.bytes().await.expect("parttable bytes");
    assert_eq!(table_back.as_ref(), table.as_slice());
}

#[tokio::test]
async fn a_capture_its_host_starts_again_is_accepted() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;

    let resp = s
        .agent_put_bytes(&path::partition_table(task_id), &mac, b"early")
        .await;
    harness::expect_status(resp, StatusCode::BAD_REQUEST).await;

    start_capture(s, task_id, &mac).await;

    let resp = s.agent_post_empty(&started_uri(task_id), &mac).await;
    harness::expect_status(resp, StatusCode::OK).await;
}

#[tokio::test]
async fn upload_partition_data_requires_a_running_task() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;

    let resp = upload_partition(s, task_id, &mac, 1, "ext4", 4, split(b"data", 2)).await;
    harness::expect_status(resp, StatusCode::BAD_REQUEST).await;
}

#[tokio::test]
async fn upload_partition_data_refuses_another_agents_mac() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;
    start_capture(s, task_id, &mac).await;
    let resp = s
        .agent_put_bytes(&path::partition_table(task_id), &mac, b"table")
        .await;
    harness::expect_status(resp, StatusCode::OK).await;

    let intruder_mac = harness::unique_mac();
    seed::host(s, &intruder_mac, DISK).await;

    let resp = upload_partition(s, task_id, &intruder_mac, 1, "ext4", 4, split(b"data", 2)).await;
    harness::expect_status(resp, StatusCode::BAD_REQUEST).await;
}

#[tokio::test]
async fn parttable_on_a_fresh_capture_wipes_the_prior_partial_captures_data() {
    let s = harness::server().await;

    let mac_a = harness::unique_mac();
    let host_a = seed::host(s, &mac_a, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;

    let first = seed::capture_task(s, host_a.id, image_id).await;
    start_capture(s, first, &mac_a).await;
    let resp = s
        .agent_put_bytes(&path::partition_table(first), &mac_a, b"table-one")
        .await;
    harness::expect_status(resp, StatusCode::OK).await;

    let comp = seed::zstd(&vec![0x33u8; 2048]).await;
    let resp = upload_partition(s, first, &mac_a, 1, "ext4", 2048, split(&comp, 512)).await;
    harness::expect_status(resp, StatusCode::OK).await;

    assert_eq!(partitions(s, image_id).await.len(), 1);
    let blob_path = s.container.image_service.get_partition_path(image_id, 1);
    assert!(tokio::fs::try_exists(&blob_path).await.unwrap());

    let mac_b = harness::unique_mac();
    let host_b = seed::host(s, &mac_b, DISK).await;
    let second = seed::capture_task(s, host_b.id, image_id).await;
    start_capture(s, second, &mac_b).await;
    let resp = s
        .agent_put_bytes(&path::partition_table(second), &mac_b, b"table-two")
        .await;
    harness::expect_status(resp, StatusCode::OK).await;

    assert!(partitions(s, image_id).await.is_empty());
    assert!(!tokio::fs::try_exists(&blob_path).await.unwrap());
    let table_now = tokio::fs::read(s.container.image_service.get_partition_table_path(image_id))
        .await
        .expect("new parttable");
    assert_eq!(table_now, b"table-two");
}

#[tokio::test]
async fn failed_on_a_capture_task_faults_the_image_and_records_the_error() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;
    start_capture(s, task_id, &mac).await;
    let resp = s
        .agent_put_bytes(&path::partition_table(task_id), &mac, b"table")
        .await;
    harness::expect_status(resp, StatusCode::OK).await;
    assert_eq!(image_status(s, image_id).await, ImageStatus::Capturing);

    let resp = s
        .agent_post_json(
            &path::failed(task_id),
            &mac,
            &json!({ "error": "partclone blew up" }),
        )
        .await;
    harness::expect_status(resp, StatusCode::OK).await;

    assert_eq!(image_status(s, image_id).await, ImageStatus::Faulted);
    assert_eq!(
        image_error(s, image_id).await.as_deref(),
        Some("partclone blew up")
    );
    assert_eq!(aggregate_state(s, task_id).await, TaskState::Failed);
}

#[tokio::test]
async fn failed_on_a_deploy_task_leaves_the_image_ready() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("dep-img"),
        b"table",
        &[(1, "ext4", &[0xAB; 256])],
    )
    .await;
    assert_eq!(image_status(s, image_id).await, ImageStatus::Ready);

    let task_id = seed::deploy_task(s, host.id, image_id).await;
    let resp = s
        .agent_post_json(
            &path::failed(task_id),
            &mac,
            &json!({ "error": "deploy failed" }),
        )
        .await;
    harness::expect_status(resp, StatusCode::OK).await;

    assert_eq!(image_status(s, image_id).await, ImageStatus::Ready);
    assert_eq!(image_error(s, image_id).await, None);
}

#[tokio::test]
async fn a_stream_that_errors_mid_upload_leaves_nothing_behind_and_cannot_be_marked_ready() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;
    start_capture(s, task_id, &mac).await;
    let resp = s
        .agent_put_bytes(&path::partition_table(task_id), &mac, b"table")
        .await;
    harness::expect_status(resp, StatusCode::OK).await;

    let chunk1 = Bytes::from_static(b"AAAAAAAAAAAAAAAA");
    let chunk2 = Bytes::from_static(b"BBBBBBBBBBBBBBBB");
    let body = stream::iter(vec![
        Ok::<Bytes, std::io::Error>(chunk1.clone()),
        Ok(chunk2.clone()),
        Err(std::io::Error::other("stream aborted")),
    ]);

    let result = s
        .http
        .put(format!(
            "{}{}?filesystem=ext4&size=32",
            s.base_url,
            path::partition_data(task_id, 1)
        ))
        .header(MAC_HEADER, &mac)
        .header("Content-Type", "application/octet-stream")
        .body(reqwest::Body::wrap_stream(body))
        .send()
        .await;

    let succeeded = matches!(&result, Ok(r) if r.status().is_success());
    assert!(!succeeded, "a truncated upload must not report success");

    let blob_path = s.container.image_service.get_partition_path(image_id, 1);
    assert!(
        !tokio::fs::try_exists(&blob_path).await.unwrap_or(false),
        "an aborted upload must leave no partition blob at the destination"
    );

    assert!(
        partitions(s, image_id).await.is_empty(),
        "no partition row is recorded when the upload stream errors"
    );

    let resp = s.agent_post_empty(&path::finished(task_id), &mac).await;
    harness::expect_status(resp, StatusCode::PRECONDITION_FAILED).await;
    assert_eq!(image_status(s, image_id).await, ImageStatus::Faulted);
    assert!(image_error(s, image_id).await.is_some());
}

#[tokio::test]
async fn cancelling_a_running_capture_from_the_dashboard_faults_the_image() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;
    start_capture(s, task_id, &mac).await;
    let resp = s
        .agent_put_bytes(&path::partition_table(task_id), &mac, b"table")
        .await;
    harness::expect_status(resp, StatusCode::OK).await;
    assert_eq!(image_status(s, image_id).await, ImageStatus::Capturing);

    let resp = s
        .ui_post("/api/ui/tasks/cancel", &json!({ "id": task_id }))
        .await;
    harness::expect_status(resp, StatusCode::OK).await;

    assert_eq!(image_status(s, image_id).await, ImageStatus::Faulted);
    assert!(image_error(s, image_id).await.is_some());
    assert_eq!(aggregate_state(s, task_id).await, TaskState::Cancelled);
}
