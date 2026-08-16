use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use imaged_core as core;
use tower::ServiceExt as _;

use core::di::DIContainer;
use core::domain::task::TaskType;

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

#[tokio::test]
async fn agent_api_contracts() {
    let (c, _guard) = install_container().await;

    partitions_are_served_to_the_owning_agent(&c).await;
    agent_identity_is_required_on_every_route().await;
    another_agents_task_is_rejected(&c).await;
    an_unready_image_is_a_precondition_failure(&c).await;
    a_read_endpoint_rejects_other_verbs(&c).await;
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
