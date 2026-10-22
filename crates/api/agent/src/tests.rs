use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::request::Builder;
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use imaged_core as core;
use tower::ServiceExt as _;

use core::di::DIContainer;
use core::domain::script::NewScript;
use core::domain::task::TaskType;
use core::test_support::container;
use imaged_model::agent::{ImageLayout, ImagePartition, MAC_HEADER, PostInstall, path, route};
use imaged_model::image::ImageStatus;
use imaged_model::script::{InputKind, ScriptInput};
use imaged_model::task::TaskState;

const MAC: &str = "aa:bb:cc:dd:ee:01";
const IDLE_MAC: &str = "aa:bb:cc:dd:ee:02";
const OTHER_MAC: &str = "aa:bb:cc:dd:ee:03";

static SEQ: AtomicU64 = AtomicU64::new(0);

fn seq() -> u64 {
    SEQ.fetch_add(1, Ordering::Relaxed)
}

fn next_mac() -> String {
    let n = seq();
    format!(
        "02:00:00:{:02x}:{:02x}:{:02x}",
        (n >> 16) & 0xff,
        (n >> 8) & 0xff,
        n & 0xff
    )
}

fn next_name(prefix: &str) -> String {
    format!("{prefix}-{}", seq())
}

fn stream_uri(disk_size_bytes: u64) -> String {
    format!("{}?disk_size_bytes={disk_size_bytes}", route::STREAM)
}

fn table_uri(task_id: i64) -> String {
    format!("{}?sector_size=512", path::partition_table(task_id))
}

fn output_uri(task_id: i64, position: i64, name: &str) -> String {
    format!("{}?name={name}", path::script_output(task_id, position))
}

fn upload_uri(task_id: i64, partition_number: i64, filesystem: &str, size: i64) -> String {
    format!(
        "{}?filesystem={filesystem}&size={size}",
        path::partition_data(task_id, partition_number)
    )
}

fn request(method: Method, uri: &str, mac: Option<&str>) -> Builder {
    let request = Request::builder().method(method).uri(uri);
    match mac {
        Some(mac) => request.header(MAC_HEADER, mac),
        None => request,
    }
}

async fn call(c: &DIContainer, request: Request<Body>) -> Response {
    core::di::scope(c.clone(), crate::router().oneshot(request))
        .await
        .unwrap()
}

async fn send(c: &DIContainer, method: Method, uri: &str, mac: Option<&str>) -> Response {
    call(c, request(method, uri, mac).body(Body::empty()).unwrap()).await
}

async fn get(c: &DIContainer, uri: &str, mac: &str) -> Response {
    send(c, Method::GET, uri, Some(mac)).await
}

async fn post(c: &DIContainer, uri: &str, mac: &str) -> Response {
    send(c, Method::POST, uri, Some(mac)).await
}

async fn post_json(c: &DIContainer, uri: &str, mac: &str, body: serde_json::Value) -> Response {
    let request = request(Method::POST, uri, Some(mac))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    call(c, request).await
}

async fn put_bytes(c: &DIContainer, uri: &str, mac: &str, body: Vec<u8>) -> Response {
    let request = request(Method::PUT, uri, Some(mac))
        .body(Body::from(body))
        .unwrap();
    call(c, request).await
}

async fn put_stream(c: &DIContainer, uri: &str, mac: &str, chunks: Vec<Vec<u8>>) -> Response {
    let body = Body::from_stream(futures::stream::iter(
        chunks
            .into_iter()
            .map(|chunk| Ok::<_, std::io::Error>(Bytes::from(chunk))),
    ));
    call(c, request(Method::PUT, uri, Some(mac)).body(body).unwrap()).await
}

async fn body(response: Response) -> Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
}

async fn error_message(response: Response) -> String {
    let error: serde_json::Value = serde_json::from_slice(&body(response).await).unwrap();
    error["error"].as_str().unwrap().to_string()
}

async fn ready_image(c: &DIContainer, name: &str) -> i64 {
    let image = c.image_repo.create_image(name.into()).await.unwrap();
    c.image_repo
        .save_partition(image.id, 1, "vfat", 1_000)
        .await
        .unwrap();
    c.image_repo
        .save_partition(image.id, 2, "ext4", 2_000)
        .await
        .unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();
    image.id
}

async fn deploy_task(c: &DIContainer, mac: &str, image_id: i64) -> i64 {
    let host = c
        .host_repo
        .upsert_host(mac.into(), Ok(1_000_000), None)
        .await
        .unwrap();
    c.task_repo
        .create(TaskType::Deploy, vec![host.id], Some(image_id))
        .await
        .unwrap()
        .id
}

async fn host_task(
    c: &DIContainer,
    mac: &str,
    task_type: TaskType,
    image_id: Option<i64>,
) -> (i64, i64) {
    let host = c
        .host_repo
        .upsert_host(mac.into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(task_type, vec![host.id], image_id)
        .await
        .unwrap();
    (host.id, task.id)
}

#[tokio::test]
async fn partitions_are_served_to_the_owning_agent() {
    let (c, _guard) = container().await;
    let c = &c;
    let image_id = ready_image(c, "served").await;
    let task_id = deploy_task(c, MAC, image_id).await;
    c.host_repo
        .upsert_host(IDLE_MAC.into(), Ok(1_000_000), None)
        .await
        .unwrap();

    let response = get(c, &path::partitions(task_id), MAC).await;
    assert_eq!(response.status(), StatusCode::OK);

    let layout: ImageLayout = serde_json::from_slice(&body(response).await).unwrap();
    assert_eq!(layout.sector_size, 512);
    let listed: Vec<(i64, &str)> = layout
        .partitions
        .iter()
        .map(|p| (p.partition_number, p.fstype.as_str()))
        .collect();
    assert_eq!(listed, vec![(1, "vfat"), (2, "ext4")]);

    let response = get(c, &path::partitions(task_id), IDLE_MAC).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn agent_identity_is_required_on_every_route() {
    let (c, _guard) = container().await;
    let c = &c;
    let routes = [
        (Method::GET, stream_uri(1024)),
        (Method::POST, route::DISCONNECT.to_string()),
        (Method::POST, path::finished(1)),
        (Method::POST, path::failed(1)),
        (Method::GET, path::partitions(1)),
        (Method::GET, path::partition_table(1)),
        (Method::GET, path::partition_data(1, 1)),
        (Method::PUT, table_uri(1)),
        (Method::PUT, path::root_partition(1, 1)),
        (Method::GET, path::scripts(1)),
        (Method::PUT, output_uri(1, 0, "script")),
        (Method::POST, path::script_finished(1, 0)),
        (Method::PUT, upload_uri(1, 1, "ext4", 1)),
    ];

    for (method, uri) in routes {
        let response = send(c, method.clone(), &uri, None).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{method} {uri} accepted a request without {MAC_HEADER}"
        );
        assert_eq!(
            error_message(response).await,
            "missing mac",
            "{method} {uri}"
        );
    }
}

#[tokio::test]
async fn another_agents_task_is_rejected() {
    let (c, _guard) = container().await;
    let c = &c;
    let mine = deploy_task(c, MAC, ready_image(c, "mine").await).await;
    let theirs = deploy_task(c, OTHER_MAC, ready_image(c, "theirs").await).await;

    for uri in [
        path::partitions(theirs),
        path::partition_table(theirs),
        path::partition_data(theirs, 1),
    ] {
        let response = get(c, &uri, MAC).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{uri} served another agent's task"
        );
    }

    let response = post(c, &path::finished(theirs), MAC).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_ne!(
        c.task_repo.get(theirs).await.unwrap().aggregate_state(),
        TaskState::Done
    );
    assert_ne!(
        c.task_repo.get(mine).await.unwrap().aggregate_state(),
        TaskState::Done
    );
}

#[tokio::test]
async fn an_unready_image_is_a_precondition_failure() {
    let (c, _guard) = container().await;
    let c = &c;
    let image = c.image_repo.create_image("unready".into()).await.unwrap();
    let task_id = deploy_task(c, MAC, image.id).await;

    for uri in [
        path::partitions(task_id),
        path::partition_table(task_id),
        path::partition_data(task_id, 1),
    ] {
        let response = get(c, &uri, MAC).await;
        assert_eq!(
            response.status(),
            StatusCode::PRECONDITION_FAILED,
            "{uri} did not map FailedPrecondition to 412"
        );
    }
}

#[tokio::test]
async fn a_read_endpoint_rejects_other_verbs() {
    let (c, _guard) = container().await;
    let c = &c;
    let task_id = deploy_task(c, MAC, ready_image(c, "verbs").await).await;
    let uri = path::partitions(task_id);

    for method in [Method::POST, Method::PUT, Method::DELETE] {
        let response = send(c, method.clone(), &uri, Some(MAC)).await;
        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} reached a read-only endpoint"
        );
    }
}

#[tokio::test]
async fn a_missing_malformed_or_unknown_agent_mac_is_handled() {
    let (c, _guard) = container().await;
    let c = &c;
    let missing = send(c, Method::GET, &path::partitions(1), None).await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

    let malformed = request(Method::GET, &path::partitions(1), None)
        .header(MAC_HEADER, HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap())
        .body(Body::empty())
        .unwrap();
    let malformed = call(c, malformed).await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let unknown = get(c, &path::partitions(1), "de:ad:be:ef:00:99").await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_host_without_an_active_task_is_a_bad_request_not_a_not_found() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    c.host_repo
        .upsert_host(mac.clone(), Ok(1_000_000), None)
        .await
        .unwrap();

    let finished = post(c, &path::finished(1), &mac).await;
    assert_eq!(finished.status(), StatusCode::BAD_REQUEST);

    let read = get(c, &path::partitions(1), &mac).await;
    assert_eq!(read.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn marking_a_task_finished_updates_rows_and_image() {
    let (c, _guard) = container().await;
    let c = &c;
    let cap_mac = next_mac();
    let cap_image = c
        .image_repo
        .create_image(next_name("finish-cap"))
        .await
        .unwrap();
    let (_ch, cap_task) = host_task(c, &cap_mac, TaskType::Capture, Some(cap_image.id)).await;
    assert_eq!(
        c.image_repo.get_status(cap_image.id).await.unwrap(),
        ImageStatus::Empty
    );
    c.image_repo
        .save_partition(cap_image.id, 1, "ext4", 4_096)
        .await
        .unwrap();
    let resp = post(c, &path::finished(cap_task), &cap_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.image_repo.get_status(cap_image.id).await.unwrap(),
        ImageStatus::Ready
    );
    let captured = c
        .image_repo
        .get_all()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.id == cap_image.id)
        .unwrap();
    assert!(captured.captured_at.is_some());
    let task = c.task_repo.get(cap_task).await.unwrap();
    assert_eq!(task.hosts[0].state, TaskState::Done);
    assert_eq!(task.aggregate_state(), TaskState::Done);

    let mc_mac = next_mac();
    let mc_image = c
        .image_repo
        .create_image(next_name("finish-mc"))
        .await
        .unwrap();
    let (_mh, mc_task) = host_task(c, &mc_mac, TaskType::Multicast, Some(mc_image.id)).await;
    let resp = post(c, &path::finished(mc_task), &mc_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.image_repo.get_status(mc_image.id).await.unwrap(),
        ImageStatus::Empty
    );
    assert_eq!(
        c.task_repo.get(mc_task).await.unwrap().aggregate_state(),
        TaskState::Done
    );

    let rb_mac = next_mac();
    let (_rh, rb_task) = host_task(c, &rb_mac, TaskType::Reboot, None).await;
    let resp = post(c, &path::finished(rb_task), &rb_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(rb_task).await.unwrap().aggregate_state(),
        TaskState::Done
    );

    let mis_mac = next_mac();
    let (_ih, mis_task) = host_task(c, &mis_mac, TaskType::Reboot, None).await;
    let resp = post(c, &path::finished(mis_task + 9999), &mis_mac).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        c.task_repo.get(mis_task).await.unwrap().aggregate_state(),
        TaskState::Pending
    );

    let a_mac = next_mac();
    let b_mac = next_mac();
    let a = c
        .host_repo
        .upsert_host(a_mac.clone(), Ok(1_000_000), None)
        .await
        .unwrap();
    let b = c
        .host_repo
        .upsert_host(b_mac.clone(), Ok(1_000_000), None)
        .await
        .unwrap();
    let multi_img = c
        .image_repo
        .create_image(next_name("finish-multi"))
        .await
        .unwrap();
    let multi = c
        .task_repo
        .create(TaskType::Multicast, vec![a.id, b.id], Some(multi_img.id))
        .await
        .unwrap()
        .id;
    let resp = post(c, &path::finished(multi), &a_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(multi).await.unwrap().aggregate_state(),
        TaskState::Running
    );
    let resp = post(c, &path::finished(multi), &b_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(multi).await.unwrap().aggregate_state(),
        TaskState::Done
    );
}

#[tokio::test]
async fn a_capture_finished_without_partitions_faults_the_image() {
    let (c, _guard) = container().await;
    let c = &c;
    let cap_mac = next_mac();
    let cap_image = c
        .image_repo
        .create_image(next_name("finish-empty-cap"))
        .await
        .unwrap();
    let (_ch, cap_task) = host_task(c, &cap_mac, TaskType::Capture, Some(cap_image.id)).await;
    let resp = post(c, &path::finished(cap_task), &cap_mac).await;
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        error_message(resp).await,
        "Capture finished without any partitions"
    );
    assert_eq!(
        c.image_repo.get_status(cap_image.id).await.unwrap(),
        ImageStatus::Faulted
    );
    let task = c.task_repo.get(cap_task).await.unwrap();
    assert_eq!(task.hosts[0].state, TaskState::Failed);
    assert_eq!(task.aggregate_state(), TaskState::Failed);
    assert!(task.hosts[0].error.is_some());
}

#[tokio::test]
async fn marking_a_task_failed_stores_the_error_and_faults_a_capture_image() {
    let (c, _guard) = container().await;
    let c = &c;
    let cap_mac = next_mac();
    let cap_image = c
        .image_repo
        .create_image(next_name("fail-cap"))
        .await
        .unwrap();
    let (_ch, cap_task) = host_task(c, &cap_mac, TaskType::Capture, Some(cap_image.id)).await;
    let resp = post_json(
        c,
        &path::failed(cap_task),
        &cap_mac,
        serde_json::json!({ "error": "disk exploded" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.image_repo.get_status(cap_image.id).await.unwrap(),
        ImageStatus::Faulted
    );
    let task = c.task_repo.get(cap_task).await.unwrap();
    assert_eq!(task.hosts[0].state, TaskState::Failed);
    assert_eq!(task.hosts[0].error.as_deref(), Some("disk exploded"));
    assert_eq!(task.aggregate_state(), TaskState::Failed);

    let mc_mac = next_mac();
    let mc_image = c
        .image_repo
        .create_image(next_name("fail-mc"))
        .await
        .unwrap();
    let (_mh, mc_task) = host_task(c, &mc_mac, TaskType::Multicast, Some(mc_image.id)).await;
    let resp = post_json(
        c,
        &path::failed(mc_task),
        &mc_mac,
        serde_json::json!({ "error": "net down" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.image_repo.get_status(mc_image.id).await.unwrap(),
        ImageStatus::Empty
    );
    assert_eq!(
        c.task_repo.get(mc_task).await.unwrap().hosts[0]
            .error
            .as_deref(),
        Some("net down")
    );

    let mis_mac = next_mac();
    let (_ih, mis_task) = host_task(c, &mis_mac, TaskType::Reboot, None).await;
    let resp = post_json(
        c,
        &path::failed(mis_task + 9999),
        &mis_mac,
        serde_json::json!({ "error": "x" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        c.task_repo.get(mis_task).await.unwrap().aggregate_state(),
        TaskState::Pending
    );

    let a_mac = next_mac();
    let b_mac = next_mac();
    let a = c
        .host_repo
        .upsert_host(a_mac.clone(), Ok(1_000_000), None)
        .await
        .unwrap();
    let b = c
        .host_repo
        .upsert_host(b_mac.clone(), Ok(1_000_000), None)
        .await
        .unwrap();
    let img = c
        .image_repo
        .create_image(next_name("fail-partial"))
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Multicast, vec![a.id, b.id], Some(img.id))
        .await
        .unwrap()
        .id;
    assert_eq!(
        post(c, &path::finished(task), &a_mac).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            c,
            &path::failed(task),
            &b_mac,
            serde_json::json!({ "error": "b failed" }),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let settled = c.task_repo.get(task).await.unwrap();
    assert_eq!(
        settled.aggregate_state(),
        TaskState::Partial,
        "{:?}",
        settled.hosts
    );
}

#[tokio::test]
async fn downloading_the_partition_table_starts_the_host_row_and_is_idempotent() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let table = b"GPT-PARTITION-TABLE".to_vec();
    let image_id = ready_image(c, &next_name("dl-parttable")).await;
    c.image_service
        .save_partition_table(image_id, &table)
        .await
        .unwrap();
    let (_h, task) = host_task(c, &mac, TaskType::Multicast, Some(image_id)).await;

    assert_eq!(
        c.task_repo.get(task).await.unwrap().hosts[0].state,
        TaskState::Pending
    );

    let resp = get(c, &path::partition_table(task), &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body(resp).await.as_ref(), table.as_slice());
    assert_eq!(
        c.task_repo.get(task).await.unwrap().hosts[0].state,
        TaskState::Running
    );

    let resp = get(c, &path::partition_table(task), &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(task).await.unwrap().hosts[0].state,
        TaskState::Running
    );
}

#[tokio::test]
async fn downloading_the_partition_table_rejects_non_restore_tasks_and_unready_images() {
    let (c, _guard) = container().await;
    let c = &c;
    let unready_mac = next_mac();
    let unready_img = c
        .image_repo
        .create_image(next_name("dl-unready"))
        .await
        .unwrap();
    let (_uh, unready_task) =
        host_task(c, &unready_mac, TaskType::Deploy, Some(unready_img.id)).await;
    let resp = get(c, &path::partition_table(unready_task), &unready_mac).await;
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);

    let cap_mac = next_mac();
    let cap_img = ready_image(c, &next_name("dl-cap")).await;
    let (_ch, cap_task) = host_task(c, &cap_mac, TaskType::Capture, Some(cap_img)).await;
    let resp = get(c, &path::partition_table(cap_task), &cap_mac).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let noimg_mac = next_mac();
    let (_nh, noimg_task) = host_task(c, &noimg_mac, TaskType::Deploy, None).await;
    let resp = get(c, &path::partition_table(noimg_task), &noimg_mac).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn downloading_partition_data_streams_the_exact_bytes_on_disk() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image_id = ready_image(c, &next_name("dl-data")).await;
    let payload: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    let partition_path = c.image_service.get_partition_path(image_id, 1);
    std::fs::create_dir_all(Path::new(&partition_path).parent().unwrap()).unwrap();
    std::fs::write(&partition_path, &payload).unwrap();
    let (_h, task) = host_task(c, &mac, TaskType::Deploy, Some(image_id)).await;

    let resp = get(c, &path::partition_data(task, 1), &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body(resp).await.as_ref(), payload.as_slice());

    let resp = get(c, &path::partition_data(task, 99), &mac).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let unready_mac = next_mac();
    let unready_img = c
        .image_repo
        .create_image(next_name("dl-data-unready"))
        .await
        .unwrap();
    let (_uh, unready_task) =
        host_task(c, &unready_mac, TaskType::Deploy, Some(unready_img.id)).await;
    let resp = get(c, &path::partition_data(unready_task, 1), &unready_mac).await;
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn downloading_partitions_matches_the_repository_in_order() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image = c
        .image_repo
        .create_image(next_name("dl-parts"))
        .await
        .unwrap();
    c.image_repo
        .save_partition(image.id, 2, "ext4", 2_048)
        .await
        .unwrap();
    c.image_repo
        .save_partition(image.id, 1, "vfat", 512)
        .await
        .unwrap();
    c.image_repo
        .save_partition(image.id, 3, "ntfs", 4_096)
        .await
        .unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();
    let (_h, task) = host_task(c, &mac, TaskType::Deploy, Some(image.id)).await;

    let resp = get(c, &path::partitions(task), &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let served: ImageLayout = serde_json::from_slice(&body(resp).await).unwrap();

    let expected = c.image_repo.get_partitions(image.id).await.unwrap();
    assert_eq!(served.partitions, expected);

    let listed: Vec<(i64, &str)> = served
        .partitions
        .iter()
        .map(|p| (p.partition_number, p.fstype.as_str()))
        .collect();
    assert_eq!(listed, vec![(1, "vfat"), (2, "ext4"), (3, "ntfs")]);
}

#[tokio::test]
async fn capturing_the_partition_table_clears_prior_capture_data_and_starts_the_host() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image = c
        .image_repo
        .create_image(next_name("cap-tbl"))
        .await
        .unwrap();
    let (host_id, task) = host_task(c, &mac, TaskType::Capture, Some(image.id)).await;

    let first_table = b"FIRST-TABLE".to_vec();
    let resp = put_bytes(c, &table_uri(task), &mac, first_table.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.image_repo.get_status(image.id).await.unwrap(),
        ImageStatus::Capturing
    );
    let table_path = c.image_service.get_partition_table_path(image.id);
    assert_eq!(std::fs::read(&table_path).unwrap(), first_table);
    let t = c.task_repo.get(task).await.unwrap();
    assert_eq!(t.hosts[0].state, TaskState::Running);
    assert_eq!(t.aggregate_state(), TaskState::Running);

    c.image_repo
        .save_partition(image.id, 1, "ext4", 42)
        .await
        .unwrap();
    let part_path = c.image_service.get_partition_path(image.id, 1);
    std::fs::create_dir_all(Path::new(&part_path).parent().unwrap()).unwrap();
    std::fs::write(&part_path, b"partial-capture").unwrap();
    assert!(Path::new(&part_path).exists());

    c.task_repo
        .mark_failed(task, host_id, "interrupted")
        .await
        .unwrap();
    c.task_repo.retry(task).await.unwrap();
    assert_eq!(
        c.task_repo.get(task).await.unwrap().aggregate_state(),
        TaskState::Pending
    );

    let second_table = b"SECOND-TABLE".to_vec();
    let resp = put_bytes(c, &table_uri(task), &mac, second_table.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(!Path::new(&part_path).exists());
    assert!(
        c.image_repo
            .get_partitions(image.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(std::fs::read(&table_path).unwrap(), second_table);

    let running_mac = next_mac();
    let running_img = c
        .image_repo
        .create_image(next_name("cap-running"))
        .await
        .unwrap();
    let (_rh, running_task) =
        host_task(c, &running_mac, TaskType::Capture, Some(running_img.id)).await;
    assert_eq!(
        put_bytes(c, &table_uri(running_task), &running_mac, b"a".to_vec())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        put_bytes(c, &table_uri(running_task), &running_mac, b"b".to_vec())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn uploading_partition_data_streams_to_disk_and_records_the_row() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image = c
        .image_repo
        .create_image(next_name("cap-upload"))
        .await
        .unwrap();
    let (_host, task) = host_task(c, &mac, TaskType::Capture, Some(image.id)).await;

    let resp = put_stream(
        c,
        &upload_uri(task, 1, "ext4", 4),
        &mac,
        vec![b"abcd".to_vec()],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = put_bytes(c, &table_uri(task), &mac, b"tbl".to_vec()).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let chunk_a = vec![0xABu8; 4096];
    let chunk_b = vec![0xCDu8; 3072];
    let mut expected = chunk_a.clone();
    expected.extend_from_slice(&chunk_b);
    let size = expected.len() as i64;
    let resp = put_stream(
        c,
        &upload_uri(task, 2, "ext4", size),
        &mac,
        vec![chunk_a, chunk_b],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let uploaded: ImagePartition = serde_json::from_slice(&body(resp).await).unwrap();

    let on_disk = std::fs::read(c.image_service.get_partition_path(image.id, 2)).unwrap();
    assert_eq!(on_disk, expected);

    let parts = c.image_repo.get_partitions(image.id).await.unwrap();
    let recorded = parts
        .into_iter()
        .find(|p| p.partition_number == 2)
        .expect("partition 2 row was recorded");
    assert_eq!(recorded.fstype, "ext4");
    assert_eq!(recorded.size_bytes, size as u64);
    assert_eq!(uploaded, recorded);

    let restore_mac = next_mac();
    let restore_img = c
        .image_repo
        .create_image(next_name("upload-restore"))
        .await
        .unwrap();
    let (_rh, restore_task) =
        host_task(c, &restore_mac, TaskType::Deploy, Some(restore_img.id)).await;
    let resp = put_stream(
        c,
        &upload_uri(restore_task, 1, "ext4", 1),
        &restore_mac,
        vec![b"x".to_vec()],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_capture_reports_the_root_partition_among_its_partitions() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image = c
        .image_repo
        .create_image(next_name("cap-root"))
        .await
        .unwrap();
    let (_host, task) = host_task(c, &mac, TaskType::Capture, Some(image.id)).await;
    let root = path::root_partition(task, 2);

    assert_eq!(
        send(c, Method::PUT, &root, Some(&mac)).await.status(),
        StatusCode::BAD_REQUEST
    );

    put_bytes(c, &table_uri(task), &mac, b"t".to_vec()).await;
    put_stream(
        c,
        &upload_uri(task, 2, "ext4", 1),
        &mac,
        vec![b"x".to_vec()],
    )
    .await;
    assert_eq!(
        send(c, Method::PUT, &path::root_partition(task, 3), Some(&mac))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(c, Method::PUT, &root, Some(&next_mac()))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        send(c, Method::PUT, &root, Some(&mac)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        c.image_repo.get(image.id).await.unwrap().root_partition,
        Some(2)
    );
}

fn script_input(name: &str, default_value: Option<&str>) -> ScriptInput {
    ScriptInput {
        name: name.to_string(),
        label: name.to_string(),
        description: None,
        kind: InputKind::Text,
        required: false,
        default_value: default_value.map(str::to_string),
        per_machine: false,
        choices: Vec::new(),
    }
}

async fn scripted_image(c: &DIContainer, scripts: &[(&str, Vec<ScriptInput>)]) -> i64 {
    let image_id = ready_image(c, &next_name("scripted")).await;
    let mut ids = Vec::new();
    for (name, inputs) in scripts {
        let script = c
            .script_repo
            .create(&NewScript {
                name: next_name(name),
                body: format!("#!/bin/sh\necho {name}"),
                inputs: inputs.clone(),
            })
            .await
            .unwrap();
        ids.push(script.id);
    }
    c.script_repo.set_for_image(image_id, &ids).await.unwrap();
    image_id
}

async fn script_runs(c: &DIContainer, task: i64, host: i64) -> Vec<(String, String, Option<i64>)> {
    c.task_repo
        .get_script_runs(task, host)
        .await
        .unwrap()
        .into_iter()
        .map(|run| (run.script_name, run.output, run.exit_code))
        .collect()
}

#[tokio::test]
async fn a_restoring_agent_fetches_its_scripts_and_reports_their_output() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image_id = scripted_image(
        c,
        &[
            (
                "first",
                vec![
                    script_input("site", Some("lab")),
                    script_input("note", None),
                ],
            ),
            ("second", vec![]),
        ],
    )
    .await;
    c.image_repo
        .set_root_partition(image_id, Some(2))
        .await
        .unwrap();
    let host = c
        .host_repo
        .upsert_host(mac.clone(), Ok(1_000_000), None)
        .await
        .unwrap()
        .id;
    let stored = std::collections::BTreeMap::from([(
        host,
        std::collections::BTreeMap::from([("note".to_string(), "hi".to_string())]),
    )]);
    let task = c
        .task_repo
        .create_with_inputs(TaskType::Deploy, vec![host], Some(image_id), &stored)
        .await
        .unwrap()
        .id;
    c.task_repo.start(task, host).await.unwrap();

    let response = get(c, &path::scripts(task), &mac).await;
    assert_eq!(response.status(), StatusCode::OK);
    let plan: PostInstall = serde_json::from_slice(&body(response).await).unwrap();
    assert_eq!(plan.root_partition, Some(2));
    assert_eq!(plan.proxy, None);
    type Listed<'a> = (i64, bool, Vec<(&'a str, &'a str)>);
    let scripts: Vec<Listed> = plan
        .scripts
        .iter()
        .map(|s| {
            (
                s.position,
                s.body.starts_with("#!/bin/sh"),
                s.inputs
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        scripts,
        vec![
            (0, true, vec![("note", "hi"), ("site", "lab")]),
            (1, true, vec![])
        ]
    );
    let first_name = plan.scripts[0].name.clone();
    let second_name = plan.scripts[1].name.clone();

    let output = |position: i64, name: &str, chunks: Vec<&[u8]>| {
        let uri = output_uri(task, position, name);
        let chunks = chunks.into_iter().map(<[u8]>::to_vec).collect();
        let mac = mac.clone();
        async move { put_stream(c, &uri, &mac, chunks).await }
    };
    let finished = |position: i64, exit_code: Option<i64>| {
        let uri = path::script_finished(task, position);
        let mac = mac.clone();
        async move { post_json(c, &uri, &mac, serde_json::json!({ "exit_code": exit_code })).await }
    };

    let attached = c.script_repo.get_for_image(image_id).await.unwrap();
    c.script_repo
        .set_for_image(image_id, &[attached[0].id])
        .await
        .unwrap();

    assert_eq!(
        output(0, &first_name, vec![b"price: \xe2\x82", b"\xac 5\n"])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(finished(0, Some(0)).await.status(), StatusCode::OK);
    assert_eq!(
        output(1, &second_name, vec![b"boom\n"]).await.status(),
        StatusCode::OK
    );
    assert_eq!(finished(1, Some(3)).await.status(), StatusCode::OK);
    assert_eq!(
        script_runs(c, task, host).await,
        vec![
            (
                first_name.clone(),
                "price: \u{20ac} 5\n".to_string(),
                Some(0)
            ),
            (second_name, "boom\n".to_string(), Some(3)),
        ]
    );

    assert_eq!(finished(1, Some(0)).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        output(-1, &first_name, vec![b"x"]).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        output(2, "..", vec![b"x"]).await.status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        output(0, &first_name, vec![b"again\n"]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        script_runs(c, task, host).await,
        vec![(first_name, "again\n".to_string(), None)]
    );
}

#[tokio::test]
async fn scripts_need_a_root_partition_and_a_started_task() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image_id = scripted_image(c, &[("only", vec![])]).await;
    let (host, task) = host_task(c, &mac, TaskType::Multicast, Some(image_id)).await;
    let scripts = path::scripts(task);

    assert_eq!(
        get(c, &scripts, &mac).await.status(),
        StatusCode::BAD_REQUEST
    );

    c.task_repo.start(task, host).await.unwrap();
    assert_eq!(
        get(c, &scripts, &mac).await.status(),
        StatusCode::PRECONDITION_FAILED
    );

    let plain_mac = next_mac();
    let plain = ready_image(c, &next_name("plain")).await;
    let (plain_host, plain_task) = host_task(c, &plain_mac, TaskType::Deploy, Some(plain)).await;
    c.task_repo.start(plain_task, plain_host).await.unwrap();
    let response = get(c, &path::scripts(plain_task), &plain_mac).await;
    assert_eq!(response.status(), StatusCode::OK);
    let plan: PostInstall = serde_json::from_slice(&body(response).await).unwrap();
    assert!(plan.scripts.is_empty());

    assert_eq!(
        get(c, &scripts, &plain_mac).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn the_stream_endpoint_requires_a_websocket_upgrade() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let response = get(c, &stream_uri(2048), &mac).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        c.host_repo.get_by_mac(&mac).await.is_err(),
        "a request that cannot be upgraded must not register a host"
    );

    let anonymous = send(c, Method::GET, &stream_uri(2048), None).await;
    assert_eq!(anonymous.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn disconnecting_deregisters_a_registered_host_and_is_safe_for_unknown_macs() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let host = c
        .host_repo
        .upsert_host(mac.clone(), Ok(1024), None)
        .await
        .unwrap();
    let _registration = c.host_registry.register(host.id);
    assert!(c.host_registry.connected_hosts().contains(&host.id));

    let resp = post(c, route::DISCONNECT, &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!c.host_registry.connected_hosts().contains(&host.id));

    let idle_mac = next_mac();
    c.host_repo
        .upsert_host(idle_mac.clone(), Ok(1_000_000), None)
        .await
        .unwrap();
    let resp = post(c, route::DISCONNECT, &idle_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = post(c, route::DISCONNECT, "de:ad:be:ef:00:98").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn no_agent_can_act_on_another_agents_task_on_any_endpoint() {
    let (c, _guard) = container().await;
    let c = &c;
    let a_mac = next_mac();
    let b_mac = next_mac();
    let a_image = ready_image(c, &next_name("iso-a")).await;
    let b_image = ready_image(c, &next_name("iso-b")).await;
    let (_ah, _a_task) = host_task(c, &a_mac, TaskType::Deploy, Some(a_image)).await;
    let (_bh, b_task) = host_task(c, &b_mac, TaskType::Deploy, Some(b_image)).await;

    for uri in [
        path::partitions(b_task),
        path::partition_table(b_task),
        path::partition_data(b_task, 1),
        path::scripts(b_task),
    ] {
        assert_eq!(
            get(c, &uri, &a_mac).await.status(),
            StatusCode::BAD_REQUEST,
            "read {uri} leaked across agents"
        );
    }

    assert_eq!(
        post(c, &path::finished(b_task), &a_mac).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            c,
            &path::failed(b_task),
            &a_mac,
            serde_json::json!({ "error": "x" }),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        put_bytes(c, &table_uri(b_task), &a_mac, b"x".to_vec())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        put_stream(
            c,
            &upload_uri(b_task, 1, "ext4", 1),
            &a_mac,
            vec![b"x".to_vec()],
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        put_stream(
            c,
            &output_uri(b_task, 0, "script"),
            &a_mac,
            vec![b"x".to_vec()],
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            c,
            &path::script_finished(b_task, 0),
            &a_mac,
            serde_json::json!({ "exit_code": 0 }),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            c,
            Method::PUT,
            &path::root_partition(b_task, 1),
            Some(&a_mac)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let b = c.task_repo.get(b_task).await.unwrap();
    assert_eq!(b.aggregate_state(), TaskState::Pending);
    assert_eq!(
        c.image_repo.get_status(b_image).await.unwrap(),
        ImageStatus::Ready
    );
}

#[tokio::test]
async fn save_partition_returns_the_partition_number_it_was_given() {
    let (c, _guard) = container().await;
    let image = c
        .image_repo
        .create_image("save-partition-bug".into())
        .await
        .unwrap();
    let returned = c
        .image_repo
        .save_partition(image.id, 3, "ext4", 100)
        .await
        .unwrap();
    assert_eq!(returned.partition_number, 3);
}

#[tokio::test]
async fn a_mac_in_any_case_or_separator_reaches_the_same_host_and_garbage_is_rejected() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let (_host, task) = host_task(c, &mac, TaskType::Reboot, None).await;

    let shouted = mac.to_ascii_uppercase().replace(':', "-");
    let response = post(c, &path::finished(task), &shouted).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(task).await.unwrap().aggregate_state(),
        TaskState::Done
    );

    let response = post(c, &path::finished(task), "not-a-mac").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error_message(response).await,
        "invalid MAC address \"not-a-mac\""
    );
}

#[tokio::test]
async fn reporting_the_same_outcome_again_is_accepted_but_a_different_one_is_not() {
    let (c, _guard) = container().await;
    let c = &c;
    let failure = || serde_json::json!({ "error": "disk exploded" });

    let done_mac = next_mac();
    let (_host, done) = host_task(c, &done_mac, TaskType::Reboot, None).await;
    for _ in 0..2 {
        let response = post(c, &path::finished(done), &done_mac).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let response = post_json(c, &path::failed(done), &done_mac, failure()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        c.task_repo.get(done).await.unwrap().aggregate_state(),
        TaskState::Done
    );

    let failed_mac = next_mac();
    let (_host, failed) = host_task(c, &failed_mac, TaskType::Reboot, None).await;
    for _ in 0..2 {
        let response = post_json(c, &path::failed(failed), &failed_mac, failure()).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let response = post(c, &path::finished(failed), &failed_mac).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let task = c.task_repo.get(failed).await.unwrap();
    assert_eq!(task.hosts[0].state, TaskState::Failed);
    assert_eq!(task.hosts[0].error.as_deref(), Some("disk exploded"));
}

#[tokio::test]
async fn the_captured_sector_size_is_recorded_and_handed_to_restores() {
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image = c
        .image_repo
        .create_image(next_name("sector-size"))
        .await
        .unwrap();
    let (_host, task) = host_task(c, &mac, TaskType::Capture, Some(image.id)).await;
    let upload = |sector_size: u32| {
        let uri = format!("{}?sector_size={sector_size}", path::partition_table(task));
        let mac = mac.clone();
        async move { put_bytes(c, &uri, &mac, b"table".to_vec()).await }
    };

    assert_eq!(upload(1000).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(upload(256).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(upload(4096).await.status(), StatusCode::OK);
    c.image_repo
        .save_partition(image.id, 1, "ext4", 4_096)
        .await
        .unwrap();
    assert_eq!(
        post(c, &path::finished(task), &mac).await.status(),
        StatusCode::OK
    );

    let restore_mac = next_mac();
    let restore = deploy_task(c, &restore_mac, image.id).await;
    let response = get(c, &path::partitions(restore), &restore_mac).await;
    assert_eq!(response.status(), StatusCode::OK);
    let layout: ImageLayout = serde_json::from_slice(&body(response).await).unwrap();
    assert_eq!(layout.sector_size, 4096);
    assert_eq!(layout.partitions.len(), 1);
}

fn choice_input(name: &str, choices: &[&str]) -> ScriptInput {
    ScriptInput {
        kind: InputKind::Choice,
        choices: choices.iter().map(|choice| choice.to_string()).collect(),
        ..script_input(name, None)
    }
}

#[tokio::test]
async fn stored_inputs_that_no_longer_fit_the_scripts_stop_the_restore_before_the_disk_is_written()
{
    let (c, _guard) = container().await;
    let c = &c;
    let mac = next_mac();
    let image_id = scripted_image(c, &[("setup", vec![script_input("mode", None)])]).await;
    c.image_repo
        .set_root_partition(image_id, Some(2))
        .await
        .unwrap();
    let host = c
        .host_repo
        .upsert_host(mac.clone(), Ok(1_000_000), None)
        .await
        .unwrap()
        .id;
    let stored = std::collections::BTreeMap::from([(
        host,
        std::collections::BTreeMap::from([("mode".to_string(), "fast".to_string())]),
    )]);
    let task = c
        .task_repo
        .create_with_inputs(TaskType::Deploy, vec![host], Some(image_id), &stored)
        .await
        .unwrap()
        .id;
    let script = c
        .script_repo
        .get_for_image(image_id)
        .await
        .unwrap()
        .remove(0);
    let change_inputs = |inputs: Vec<ScriptInput>| {
        let id = script.id;
        let changed = NewScript {
            name: script.name.clone(),
            body: script.body.clone(),
            inputs,
        };
        async move { c.script_repo.update(id, &changed).await.unwrap() }
    };
    let refusal = |uri: String| {
        let mac = mac.clone();
        async move {
            let response = get(c, &uri, &mac).await;
            assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED, "{uri}");
            error_message(response).await
        }
    };

    assert_eq!(
        get(c, &path::partitions(task), &mac).await.status(),
        StatusCode::OK
    );

    change_inputs(vec![choice_input("mode", &["safe", "slow"])]).await;
    let reason = refusal(path::partitions(task)).await;
    assert_eq!(
        reason,
        format!(
            "script `{}` no longer fits the inputs stored with this task: input `mode` must be one of safe, slow; start a new deployment",
            script.name
        )
    );

    c.task_repo.start(task, host).await.unwrap();
    change_inputs(vec![
        script_input("mode", None),
        ScriptInput {
            required: true,
            ..script_input("site", None)
        },
    ])
    .await;
    let reason = refusal(path::scripts(task)).await;
    assert!(
        reason.ends_with("input `site` is required; start a new deployment"),
        "{reason}"
    );
    assert_eq!(
        refusal(path::partitions(task)).await,
        reason,
        "the layout and the scripts give the same reason"
    );
    assert!(script_runs(c, task, host).await.is_empty());
}
