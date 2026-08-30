use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, StatusCode};
use bytes::Bytes;
use imaged_core as core;
use tower::ServiceExt as _;

use core::di::DIContainer;
use core::domain::image::ImageStatus;
use core::domain::task::{TaskState, TaskType};

use crate::model;

const MAC: &str = "aa:bb:cc:dd:ee:01";
const IDLE_MAC: &str = "aa:bb:cc:dd:ee:02";
const OTHER_MAC: &str = "aa:bb:cc:dd:ee:03";
const UNREADY_MAC: &str = "aa:bb:cc:dd:ee:04";
const VERB_MAC: &str = "aa:bb:cc:dd:ee:05";

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Installs the container process-wide because `dioxus` runs server-function
/// handlers on its own task pool, where a `di::scope` task-local never reaches
/// them. `init_container` panics if called twice, so every case below shares
/// one test and one sqlx pool bound to a single runtime.
async fn install_container() -> (DIContainer, TestDir) {
    let dir = std::env::temp_dir().join(format!("imaged-agent-test-{}", std::process::id()));
    let c = core::build_test_container(&dir).await;
    core::di::init_container(c.clone());
    (c, TestDir(dir))
}

static DB_ID: AtomicU64 = AtomicU64::new(0);

async fn container() -> (DIContainer, TestDir) {
    let id = DB_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("imaged-agentbug-{}-{}", std::process::id(), id));
    let c = core::build_test_container(&dir).await;
    (c, TestDir(dir))
}

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

fn agent_router() -> axum::Router {
    dioxus_server::ServerFunction::collect()
        .into_iter()
        .filter(|f| f.path().starts_with("/api/client"))
        .fold(axum::Router::new(), |router, f| {
            router.route(f.path(), f.method_router())
        })
        .with_state(dioxus_server::FullstackState::headless())
}

async fn send(method: Method, uri: &str, mac: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(mac) = mac {
        request = request.header("X-Agent-Mac", mac);
    }
    agent_router()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn get(uri: &str, mac: &str) -> axum::response::Response {
    send(Method::GET, uri, Some(mac)).await
}

async fn post(uri: &str, mac: &str) -> axum::response::Response {
    send(Method::POST, uri, Some(mac)).await
}

async fn post_json(uri: &str, mac: &str, body: serde_json::Value) -> axum::response::Response {
    let request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("X-Agent-Mac", mac)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    agent_router().oneshot(request).await.unwrap()
}

// A `Bytes` server-fn argument is `Serialize`, so it does not travel as a raw
// octet-stream body the way a `ByteStream` argument does: it is JSON, in the
// same argument-name envelope every other non-path argument uses.
async fn put_bytes(uri: &str, mac: &str, body: Vec<u8>) -> axum::response::Response {
    let payload = serde_json::json!({ "body": body });
    let request = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header("X-Agent-Mac", mac)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    agent_router().oneshot(request).await.unwrap()
}

async fn put_stream(uri: &str, mac: &str, chunks: Vec<Vec<u8>>) -> axum::response::Response {
    let body = Body::from_stream(futures::stream::iter(
        chunks
            .into_iter()
            .map(|c| Ok::<_, std::io::Error>(Bytes::from(c))),
    ));
    let request = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header("X-Agent-Mac", mac)
        .header("content-type", "application/octet-stream")
        .body(body)
        .unwrap();
    agent_router().oneshot(request).await.unwrap()
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
        .upsert_host(mac.into(), 1_000_000, None)
        .await
        .unwrap();
    c.task_repo
        .create(TaskType::Multicast, vec![host.id], Some(image_id))
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
        .upsert_host(mac.into(), 1_000_000, None)
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
async fn agent_api_contracts() {
    let (c, _guard) = install_container().await;

    partitions_are_served_to_the_owning_agent(&c).await;
    agent_identity_is_required_on_every_route().await;
    another_agents_task_is_rejected(&c).await;
    an_unready_image_is_a_precondition_failure(&c).await;
    a_read_endpoint_rejects_other_verbs(&c).await;
    a_missing_malformed_or_unknown_agent_mac_is_handled().await;
    a_host_without_an_active_task_is_a_bad_request_not_a_not_found(&c).await;
    marking_a_task_finished_updates_rows_and_image(&c).await;
    a_capture_finished_without_partitions_faults_the_image(&c).await;
    marking_a_task_failed_stores_the_error_and_faults_a_capture_image(&c).await;
    downloading_the_partition_table_starts_the_host_row_and_is_idempotent(&c).await;
    downloading_the_partition_table_rejects_non_restore_tasks_and_unready_images(&c).await;
    downloading_partition_data_streams_the_exact_bytes_on_disk(&c).await;
    downloading_partitions_matches_the_repository_in_order(&c).await;
    capturing_the_partition_table_clears_prior_capture_data_and_starts_the_host(&c).await;
    uploading_partition_data_streams_to_disk_and_records_the_row(&c).await;
    starting_a_stream_registers_the_host_and_refuses_a_second_connection(&c).await;
    disconnecting_deregisters_a_registered_host_and_is_safe_for_unknown_macs(&c).await;
    no_agent_can_act_on_another_agents_task_on_any_endpoint(&c).await;
}

async fn partitions_are_served_to_the_owning_agent(c: &DIContainer) {
    let image_id = ready_image(c, "served").await;
    let task_id = deploy_task(c, MAC, image_id).await;
    c.host_repo
        .upsert_host(IDLE_MAC.into(), 1_000_000, None)
        .await
        .unwrap();

    let response = get(&format!("/api/client/tasks/{task_id}/partitions"), MAC).await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let partitions: Vec<model::ImagePartition> = serde_json::from_slice(&body).unwrap();
    let listed: Vec<(i64, &str)> = partitions
        .iter()
        .map(|p| (p.partition_number, p.fstype.as_str()))
        .collect();
    assert_eq!(listed, vec![(1, "vfat"), (2, "ext4")]);

    let response = get(&format!("/api/client/tasks/{task_id}/partitions"), IDLE_MAC).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

async fn agent_identity_is_required_on_every_route() {
    let routes = [
        (Method::GET, "/api/client/stream?disk_size_bytes=1024"),
        (Method::POST, "/api/client/stream/disconnect"),
        (Method::POST, "/api/client/tasks/1/finished"),
        (Method::POST, "/api/client/tasks/1/failed"),
        (Method::GET, "/api/client/tasks/1/partitions"),
        (Method::GET, "/api/client/tasks/1/parttable"),
        (Method::GET, "/api/client/tasks/1/partitions/1/data"),
        (Method::PUT, "/api/client/tasks/1/parttable"),
        (
            Method::PUT,
            "/api/client/tasks/1/partitions/1/data?filesystem=ext4&size=1",
        ),
    ];

    for (method, uri) in routes {
        let response = send(method.clone(), uri, None).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{method} {uri} accepted a request without X-Agent-Mac"
        );
    }
}

async fn another_agents_task_is_rejected(c: &DIContainer) {
    let mine = deploy_task(c, MAC, ready_image(c, "mine").await).await;
    let theirs = deploy_task(c, OTHER_MAC, ready_image(c, "theirs").await).await;

    for uri in [
        format!("/api/client/tasks/{theirs}/partitions"),
        format!("/api/client/tasks/{theirs}/parttable"),
        format!("/api/client/tasks/{theirs}/partitions/1/data"),
    ] {
        let response = get(&uri, MAC).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{uri} served another agent's task"
        );
    }

    let response = send(
        Method::POST,
        &format!("/api/client/tasks/{theirs}/finished"),
        Some(MAC),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_ne!(
        c.task_repo.get(theirs).await.unwrap().aggregate_state(),
        core::domain::task::TaskState::Done
    );
    assert_ne!(
        c.task_repo.get(mine).await.unwrap().aggregate_state(),
        core::domain::task::TaskState::Done
    );
}

async fn an_unready_image_is_a_precondition_failure(c: &DIContainer) {
    let image = c.image_repo.create_image("unready".into()).await.unwrap();
    let task_id = deploy_task(c, UNREADY_MAC, image.id).await;

    for uri in [
        format!("/api/client/tasks/{task_id}/partitions"),
        format!("/api/client/tasks/{task_id}/parttable"),
        format!("/api/client/tasks/{task_id}/partitions/1/data"),
    ] {
        let response = get(&uri, UNREADY_MAC).await;
        assert_eq!(
            response.status(),
            StatusCode::PRECONDITION_FAILED,
            "{uri} did not map FailedPrecondition to 412"
        );
    }
}

async fn a_read_endpoint_rejects_other_verbs(c: &DIContainer) {
    let task_id = deploy_task(c, VERB_MAC, ready_image(c, "verbs").await).await;
    let uri = format!("/api/client/tasks/{task_id}/partitions");

    for method in [Method::POST, Method::PUT, Method::DELETE] {
        let response = send(method.clone(), &uri, Some(VERB_MAC)).await;
        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} reached a read-only endpoint"
        );
    }
}

async fn a_missing_malformed_or_unknown_agent_mac_is_handled() {
    let missing = send(Method::GET, "/api/client/tasks/1/partitions", None).await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

    let malformed = Request::builder()
        .method(Method::GET)
        .uri("/api/client/tasks/1/partitions")
        .header("X-Agent-Mac", HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap())
        .body(Body::empty())
        .unwrap();
    let malformed = agent_router().oneshot(malformed).await.unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let unknown = get("/api/client/tasks/1/partitions", "de:ad:be:ef:00:99").await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

async fn a_host_without_an_active_task_is_a_bad_request_not_a_not_found(c: &DIContainer) {
    let mac = next_mac();
    c.host_repo
        .upsert_host(mac.clone(), 1_000_000, None)
        .await
        .unwrap();

    let finished = post("/api/client/tasks/1/finished", &mac).await;
    assert_eq!(finished.status(), StatusCode::BAD_REQUEST);

    let read = get("/api/client/tasks/1/partitions", &mac).await;
    assert_eq!(read.status(), StatusCode::BAD_REQUEST);
}

async fn marking_a_task_finished_updates_rows_and_image(c: &DIContainer) {
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
    let resp = post(&format!("/api/client/tasks/{cap_task}/finished"), &cap_mac).await;
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
    let resp = post(&format!("/api/client/tasks/{mc_task}/finished"), &mc_mac).await;
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
    let resp = post(&format!("/api/client/tasks/{rb_task}/finished"), &rb_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(rb_task).await.unwrap().aggregate_state(),
        TaskState::Done
    );

    let mis_mac = next_mac();
    let (_ih, mis_task) = host_task(c, &mis_mac, TaskType::Reboot, None).await;
    let resp = post(
        &format!("/api/client/tasks/{}/finished", mis_task + 9999),
        &mis_mac,
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
        .upsert_host(a_mac.clone(), 1_000_000, None)
        .await
        .unwrap();
    let b = c
        .host_repo
        .upsert_host(b_mac.clone(), 1_000_000, None)
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
    let resp = post(&format!("/api/client/tasks/{multi}/finished"), &a_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(multi).await.unwrap().aggregate_state(),
        TaskState::Running
    );
    let resp = post(&format!("/api/client/tasks/{multi}/finished"), &b_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(multi).await.unwrap().aggregate_state(),
        TaskState::Done
    );
}

async fn a_capture_finished_without_partitions_faults_the_image(c: &DIContainer) {
    let cap_mac = next_mac();
    let cap_image = c
        .image_repo
        .create_image(next_name("finish-empty-cap"))
        .await
        .unwrap();
    let (_ch, cap_task) = host_task(c, &cap_mac, TaskType::Capture, Some(cap_image.id)).await;
    let resp = post(&format!("/api/client/tasks/{cap_task}/finished"), &cap_mac).await;
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        c.image_repo.get_status(cap_image.id).await.unwrap(),
        ImageStatus::Faulted
    );
    let task = c.task_repo.get(cap_task).await.unwrap();
    assert_eq!(task.hosts[0].state, TaskState::Failed);
    assert_eq!(task.aggregate_state(), TaskState::Failed);
    assert!(task.hosts[0].error.is_some());
}

async fn marking_a_task_failed_stores_the_error_and_faults_a_capture_image(c: &DIContainer) {
    let cap_mac = next_mac();
    let cap_image = c
        .image_repo
        .create_image(next_name("fail-cap"))
        .await
        .unwrap();
    let (_ch, cap_task) = host_task(c, &cap_mac, TaskType::Capture, Some(cap_image.id)).await;
    let resp = post_json(
        &format!("/api/client/tasks/{cap_task}/failed"),
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
        &format!("/api/client/tasks/{mc_task}/failed"),
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
        &format!("/api/client/tasks/{}/failed", mis_task + 9999),
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
        .upsert_host(a_mac.clone(), 1_000_000, None)
        .await
        .unwrap();
    let b = c
        .host_repo
        .upsert_host(b_mac.clone(), 1_000_000, None)
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
        post(&format!("/api/client/tasks/{task}/finished"), &a_mac)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &format!("/api/client/tasks/{task}/failed"),
            &b_mac,
            serde_json::json!({ "error": "b failed" }),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        c.task_repo.get(task).await.unwrap().aggregate_state(),
        TaskState::Partial
    );
}

async fn downloading_the_partition_table_starts_the_host_row_and_is_idempotent(c: &DIContainer) {
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

    let resp = get(&format!("/api/client/tasks/{task}/parttable"), &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let decoded: Vec<u8> = serde_json::from_slice(&body).unwrap();
    assert_eq!(decoded, table);
    assert_eq!(
        c.task_repo.get(task).await.unwrap().hosts[0].state,
        TaskState::Running
    );

    let resp = get(&format!("/api/client/tasks/{task}/parttable"), &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        c.task_repo.get(task).await.unwrap().hosts[0].state,
        TaskState::Running
    );
}

async fn downloading_the_partition_table_rejects_non_restore_tasks_and_unready_images(
    c: &DIContainer,
) {
    let unready_mac = next_mac();
    let unready_img = c
        .image_repo
        .create_image(next_name("dl-unready"))
        .await
        .unwrap();
    let (_uh, unready_task) =
        host_task(c, &unready_mac, TaskType::Multicast, Some(unready_img.id)).await;
    let resp = get(
        &format!("/api/client/tasks/{unready_task}/parttable"),
        &unready_mac,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);

    let cap_mac = next_mac();
    let cap_img = ready_image(c, &next_name("dl-cap")).await;
    let (_ch, cap_task) = host_task(c, &cap_mac, TaskType::Capture, Some(cap_img)).await;
    let resp = get(&format!("/api/client/tasks/{cap_task}/parttable"), &cap_mac).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let noimg_mac = next_mac();
    let (_nh, noimg_task) = host_task(c, &noimg_mac, TaskType::Multicast, None).await;
    let resp = get(
        &format!("/api/client/tasks/{noimg_task}/parttable"),
        &noimg_mac,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

async fn downloading_partition_data_streams_the_exact_bytes_on_disk(c: &DIContainer) {
    let mac = next_mac();
    let image_id = ready_image(c, &next_name("dl-data")).await;
    let payload: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    let path = c.image_service.get_partition_path(image_id, 1);
    std::fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
    std::fs::write(&path, &payload).unwrap();
    let (_h, task) = host_task(c, &mac, TaskType::Multicast, Some(image_id)).await;

    let resp = get(
        &format!("/api/client/tasks/{task}/partitions/1/data"),
        &mac,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), payload.as_slice());

    let resp = get(
        &format!("/api/client/tasks/{task}/partitions/99/data"),
        &mac,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let unready_mac = next_mac();
    let unready_img = c
        .image_repo
        .create_image(next_name("dl-data-unready"))
        .await
        .unwrap();
    let (_uh, unready_task) =
        host_task(c, &unready_mac, TaskType::Multicast, Some(unready_img.id)).await;
    let resp = get(
        &format!("/api/client/tasks/{unready_task}/partitions/1/data"),
        &unready_mac,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
}

async fn downloading_partitions_matches_the_repository_in_order(c: &DIContainer) {
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
    let (_h, task) = host_task(c, &mac, TaskType::Multicast, Some(image.id)).await;

    let resp = get(&format!("/api/client/tasks/{task}/partitions"), &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let served: Vec<model::ImagePartition> = serde_json::from_slice(&body).unwrap();

    let expected: Vec<model::ImagePartition> = c
        .image_repo
        .get_partitions(image.id)
        .await
        .unwrap()
        .into_iter()
        .map(Into::into)
        .collect();
    assert_eq!(served, expected);

    let listed: Vec<(i64, &str)> = served
        .iter()
        .map(|p| (p.partition_number, p.fstype.as_str()))
        .collect();
    assert_eq!(listed, vec![(1, "vfat"), (2, "ext4"), (3, "ntfs")]);
}

async fn capturing_the_partition_table_clears_prior_capture_data_and_starts_the_host(
    c: &DIContainer,
) {
    let mac = next_mac();
    let image = c
        .image_repo
        .create_image(next_name("cap-tbl"))
        .await
        .unwrap();
    let (host_id, task) = host_task(c, &mac, TaskType::Capture, Some(image.id)).await;

    let first_table = b"FIRST-TABLE".to_vec();
    let resp = put_bytes(
        &format!("/api/client/tasks/{task}/parttable"),
        &mac,
        first_table.clone(),
    )
    .await;
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
    let resp = put_bytes(
        &format!("/api/client/tasks/{task}/parttable"),
        &mac,
        second_table.clone(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(!Path::new(&part_path).exists());
    assert!(c.image_repo.get_partitions(image.id).await.unwrap().is_empty());
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
        put_bytes(
            &format!("/api/client/tasks/{running_task}/parttable"),
            &running_mac,
            b"a".to_vec(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        put_bytes(
            &format!("/api/client/tasks/{running_task}/parttable"),
            &running_mac,
            b"b".to_vec(),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

async fn uploading_partition_data_streams_to_disk_and_records_the_row(c: &DIContainer) {
    let mac = next_mac();
    let image = c
        .image_repo
        .create_image(next_name("cap-upload"))
        .await
        .unwrap();
    let (_host, task) = host_task(c, &mac, TaskType::Capture, Some(image.id)).await;

    let resp = put_stream(
        &format!("/api/client/tasks/{task}/partitions/1/data?filesystem=ext4&size=4"),
        &mac,
        vec![b"abcd".to_vec()],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = put_bytes(
        &format!("/api/client/tasks/{task}/parttable"),
        &mac,
        b"tbl".to_vec(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let chunk_a = vec![0xABu8; 4096];
    let chunk_b = vec![0xCDu8; 3072];
    let mut expected = chunk_a.clone();
    expected.extend_from_slice(&chunk_b);
    let size = expected.len() as i64;
    let resp = put_stream(
        &format!("/api/client/tasks/{task}/partitions/2/data?filesystem=ext4&size={size}"),
        &mac,
        vec![chunk_a, chunk_b],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let on_disk = std::fs::read(c.image_service.get_partition_path(image.id, 2)).unwrap();
    assert_eq!(on_disk, expected);

    let parts = c.image_repo.get_partitions(image.id).await.unwrap();
    let recorded = parts
        .iter()
        .find(|p| p.partition_number == 2)
        .expect("partition 2 row was recorded");
    assert_eq!(recorded.fstype, "ext4");
    assert_eq!(recorded.size_bytes, size as u64);

    let mc_mac = next_mac();
    let mc_img = c
        .image_repo
        .create_image(next_name("upload-mc"))
        .await
        .unwrap();
    let (_mh, mc_task) = host_task(c, &mc_mac, TaskType::Multicast, Some(mc_img.id)).await;
    let resp = put_stream(
        &format!("/api/client/tasks/{mc_task}/partitions/1/data?filesystem=ext4&size=1"),
        &mc_mac,
        vec![b"x".to_vec()],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

async fn starting_a_stream_registers_the_host_and_refuses_a_second_connection(c: &DIContainer) {
    let mac = next_mac();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/client/stream?disk_size_bytes=2048")
        .header("X-Agent-Mac", mac.as_str())
        .header("X-Agent-Ip", "10.1.2.3")
        .body(Body::empty())
        .unwrap();
    let response = agent_router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let host = c.host_repo.get_by_mac(&mac).await.unwrap();
    assert_eq!(host.ip.as_deref(), Some("10.1.2.3"));
    assert!(
        c.host_registry
            .get_current_state()
            .iter()
            .any(|e| e.id == host.id)
    );

    let second = get("/api/client/stream?disk_size_bytes=2048", &mac).await;
    assert_eq!(second.status(), StatusCode::PRECONDITION_FAILED);

    drop(response);

    let mac2 = next_mac();
    let response = get("/api/client/stream?disk_size_bytes=4096", &mac2).await;
    assert_eq!(response.status(), StatusCode::OK);
    let host2 = c.host_repo.get_by_mac(&mac2).await.unwrap();
    assert_eq!(host2.ip, None);
    assert!(
        c.host_registry
            .get_current_state()
            .iter()
            .any(|e| e.id == host2.id)
    );
    drop(response);
}

async fn disconnecting_deregisters_a_registered_host_and_is_safe_for_unknown_macs(c: &DIContainer) {
    let mac = next_mac();
    let response = get("/api/client/stream?disk_size_bytes=1024", &mac).await;
    assert_eq!(response.status(), StatusCode::OK);
    let host = c.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(
        c.host_registry
            .get_current_state()
            .iter()
            .any(|e| e.id == host.id)
    );

    let resp = post("/api/client/stream/disconnect", &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        !c.host_registry
            .get_current_state()
            .iter()
            .any(|e| e.id == host.id)
    );
    drop(response);

    let idle_mac = next_mac();
    c.host_repo
        .upsert_host(idle_mac.clone(), 1_000_000, None)
        .await
        .unwrap();
    let resp = post("/api/client/stream/disconnect", &idle_mac).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = post("/api/client/stream/disconnect", "de:ad:be:ef:00:98").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn no_agent_can_act_on_another_agents_task_on_any_endpoint(c: &DIContainer) {
    let a_mac = next_mac();
    let b_mac = next_mac();
    let a_image = ready_image(c, &next_name("iso-a")).await;
    let b_image = ready_image(c, &next_name("iso-b")).await;
    let (_ah, _a_task) = host_task(c, &a_mac, TaskType::Multicast, Some(a_image)).await;
    let (_bh, b_task) = host_task(c, &b_mac, TaskType::Multicast, Some(b_image)).await;

    for uri in [
        format!("/api/client/tasks/{b_task}/partitions"),
        format!("/api/client/tasks/{b_task}/parttable"),
        format!("/api/client/tasks/{b_task}/partitions/1/data"),
    ] {
        assert_eq!(
            get(&uri, &a_mac).await.status(),
            StatusCode::BAD_REQUEST,
            "read {uri} leaked across agents"
        );
    }

    assert_eq!(
        post(&format!("/api/client/tasks/{b_task}/finished"), &a_mac)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &format!("/api/client/tasks/{b_task}/failed"),
            &a_mac,
            serde_json::json!({ "error": "x" }),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        put_bytes(
            &format!("/api/client/tasks/{b_task}/parttable"),
            &a_mac,
            b"x".to_vec(),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        put_stream(
            &format!("/api/client/tasks/{b_task}/partitions/1/data?filesystem=ext4&size=1"),
            &a_mac,
            vec![b"x".to_vec()],
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
