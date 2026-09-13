use bytes::Bytes;
use futures::stream;
use reqwest::StatusCode;
use serde_json::json;

use imaged_core::domain::image::ImageStatus;
use imaged_core::domain::task::TaskState;
use imaged_e2e::harness::{self, seed};

const DISK: u64 = 8 * 1024 * 1024 * 1024;

async fn create_empty_image(s: &harness::TestServer, name: &str) -> i64 {
    s.container
        .image_repo
        .create_image(name.to_string())
        .await
        .expect("create empty image")
        .id
}

async fn image_status(s: &harness::TestServer, image_id: i64) -> ImageStatus {
    s.container
        .image_repo
        .get_status(image_id)
        .await
        .expect("image status")
}

async fn captured_at_is_set(s: &harness::TestServer, image_id: i64) -> bool {
    image_field(s, image_id, |i| i.captured_at.is_some()).await
}

async fn image_error(s: &harness::TestServer, image_id: i64) -> Option<String> {
    image_field(s, image_id, |i| i.error.clone()).await
}

async fn image_field<T>(
    s: &harness::TestServer,
    image_id: i64,
    f: impl Fn(&imaged_core::domain::image::Image) -> T,
) -> T {
    let images = s.container.image_repo.get_all().await.expect("images");
    let image = images.iter().find(|i| i.id == image_id).expect("image row");
    f(image)
}

async fn aggregate_state(s: &harness::TestServer, task_id: i64) -> TaskState {
    s.container
        .task_repo
        .get(task_id)
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
            "/api/client/tasks/{task_id}/partitions/{number}/data?filesystem={fstype}&size={size}"
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
    let resp = s
        .agent_put_bytes_arg(
            &format!("/api/client/tasks/{task_id}/parttable"),
            &mac,
            "body",
            &table,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
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
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = upload_partition(s, task_id, &mac, 2, "xfs", plain2.len(), split(&comp2, 700)).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let parts = s
        .container
        .image_repo
        .get_partitions(image_id)
        .await
        .unwrap();
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

    let resp = s
        .agent_post_empty(&format!("/api/client/tasks/{task_id}/finished"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(image_status(s, image_id).await, ImageStatus::Ready);
    assert!(captured_at_is_set(s, image_id).await);

    let other_mac = harness::unique_mac();
    let other = seed::host(s, &other_mac, DISK).await;
    let deploy_id = seed::deploy_task(s, other.id, image_id).await;

    let resp = s
        .agent_get(
            &format!("/api/client/tasks/{deploy_id}/partitions/1/data"),
            &other_mac,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let downloaded = resp.bytes().await.expect("download p1");
    assert_eq!(downloaded.as_ref(), comp1.as_slice());
    assert_eq!(seed::unzstd(&downloaded).await, plain1);

    let resp = s
        .agent_get(
            &format!("/api/client/tasks/{deploy_id}/parttable"),
            &other_mac,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let table_back: Vec<u8> = resp.json().await.expect("parttable json");
    assert_eq!(table_back, table);
}

#[tokio::test]
async fn parttable_requires_a_pending_task() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;
    let path = format!("/api/client/tasks/{task_id}/parttable");

    let resp = s.agent_put_bytes_arg(&path, &mac, "body", b"first").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = s.agent_put_bytes_arg(&path, &mac, "body", b"second").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_partition_data_requires_a_running_task() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;

    let resp = upload_partition(s, task_id, &mac, 1, "ext4", 4, split(b"data", 2)).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_partition_data_rejects_a_wrong_content_type() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;
    let resp = s
        .agent_put_bytes_arg(
            &format!("/api/client/tasks/{task_id}/parttable"),
            &mac,
            "body",
            b"table",
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = s
        .http
        .put(format!(
            "{}/api/client/tasks/{task_id}/partitions/1/data?filesystem=ext4&size=4",
            s.base_url
        ))
        .header("X-Agent-Mac", &mac)
        .header("Content-Type", "text/plain")
        .body(Bytes::from_static(b"data"))
        .send()
        .await
        .expect("wrong content-type send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_partition_data_refuses_another_agents_mac() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;
    let task_id = seed::capture_task(s, host.id, image_id).await;
    let resp = s
        .agent_put_bytes_arg(
            &format!("/api/client/tasks/{task_id}/parttable"),
            &mac,
            "body",
            b"table",
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let intruder_mac = harness::unique_mac();
    seed::host(s, &intruder_mac, DISK).await;

    let resp = upload_partition(s, task_id, &intruder_mac, 1, "ext4", 4, split(b"data", 2)).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn parttable_on_a_fresh_capture_wipes_the_prior_partial_captures_data() {
    let s = harness::server().await;

    let mac_a = harness::unique_mac();
    let host_a = seed::host(s, &mac_a, DISK).await;
    let image_id = create_empty_image(s, &harness::unique_name("cap-img")).await;

    let first = seed::capture_task(s, host_a.id, image_id).await;
    let resp = s
        .agent_put_bytes_arg(
            &format!("/api/client/tasks/{first}/parttable"),
            &mac_a,
            "body",
            b"table-one",
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let comp = seed::zstd(&vec![0x33u8; 2048]).await;
    let resp = upload_partition(s, first, &mac_a, 1, "ext4", 2048, split(&comp, 512)).await;
    assert_eq!(resp.status(), StatusCode::OK);

    assert_eq!(
        s.container
            .image_repo
            .get_partitions(image_id)
            .await
            .unwrap()
            .len(),
        1
    );
    let blob_path = s.container.image_service.get_partition_path(image_id, 1);
    assert!(tokio::fs::try_exists(&blob_path).await.unwrap());

    let mac_b = harness::unique_mac();
    let host_b = seed::host(s, &mac_b, DISK).await;
    let second = seed::capture_task(s, host_b.id, image_id).await;
    let resp = s
        .agent_put_bytes_arg(
            &format!("/api/client/tasks/{second}/parttable"),
            &mac_b,
            "body",
            b"table-two",
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(
        s.container
            .image_repo
            .get_partitions(image_id)
            .await
            .unwrap()
            .is_empty()
    );
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
    let resp = s
        .agent_put_bytes_arg(
            &format!("/api/client/tasks/{task_id}/parttable"),
            &mac,
            "body",
            b"table",
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(image_status(s, image_id).await, ImageStatus::Capturing);

    let resp = s
        .http
        .post(format!("{}/api/client/tasks/{task_id}/failed", s.base_url))
        .header("X-Agent-Mac", &mac)
        .json(&json!({ "error": "partclone blew up" }))
        .send()
        .await
        .expect("failed send");
    assert_eq!(resp.status(), StatusCode::OK);

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
        .http
        .post(format!("{}/api/client/tasks/{task_id}/failed", s.base_url))
        .header("X-Agent-Mac", &mac)
        .json(&json!({ "error": "deploy failed" }))
        .send()
        .await
        .expect("failed send");
    assert_eq!(resp.status(), StatusCode::OK);

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
    let resp = s
        .agent_put_bytes_arg(
            &format!("/api/client/tasks/{task_id}/parttable"),
            &mac,
            "body",
            b"table",
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

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
            "{}/api/client/tasks/{task_id}/partitions/1/data?filesystem=ext4&size=32",
            s.base_url
        ))
        .header("X-Agent-Mac", &mac)
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
        s.container
            .image_repo
            .get_partitions(image_id)
            .await
            .unwrap()
            .is_empty(),
        "no partition row is recorded when the upload stream errors"
    );

    let resp = s
        .agent_post_empty(&format!("/api/client/tasks/{task_id}/finished"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
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
    let resp = s
        .agent_put_bytes_arg(
            &format!("/api/client/tasks/{task_id}/parttable"),
            &mac,
            "body",
            b"table",
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(image_status(s, image_id).await, ImageStatus::Capturing);

    let resp = s
        .ui_post("/api/ui/tasks/cancel", &json!({ "id": task_id }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    assert_eq!(image_status(s, image_id).await, ImageStatus::Faulted);
    assert!(image_error(s, image_id).await.is_some());
    assert_eq!(aggregate_state(s, task_id).await, TaskState::Cancelled);
}
