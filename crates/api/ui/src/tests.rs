use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use imaged_core as core;
use tower::ServiceExt as _;

use core::di::DIContainer;
use core::domain::task::TaskType;

use crate::model;

static DB_ID: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn install_container() -> (DIContainer, TestDir) {
    let dir = std::env::temp_dir().join(format!("imaged-ui-router-test-{}", std::process::id()));
    let c = core::build_test_container(&dir).await;
    core::di::init_container(c.clone());
    (c, TestDir(dir))
}

async fn container() -> (DIContainer, TestDir) {
    let id = DB_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("imaged-test-{}-{}", std::process::id(), id));
    let c = core::build_test_container(&dir).await;
    (c, TestDir(dir))
}

// ----------------------------------------------------------------------------
// hosts
// ----------------------------------------------------------------------------

#[tokio::test]
async fn update_host_name_renames() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:01".into(), 1_000_000, None)
        .await
        .unwrap();

    let updated = core::di::scope(
        c.clone(),
        crate::hosts::update_host_name(model::UpdateName {
            id: host.id,
            new_name: "renamed".into(),
        }),
    )
    .await
    .unwrap();

    assert_eq!(updated.id, host.id);
    assert_eq!(updated.name, "renamed");
}

#[tokio::test]
async fn delete_host_ok_without_tasks() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:02".into(), 1_000_000, None)
        .await
        .unwrap();

    core::di::scope(c.clone(), crate::hosts::delete_host(host.id))
        .await
        .unwrap();

    let hosts = core::di::scope(c.clone(), crate::hosts::get_all_hosts())
        .await
        .unwrap();
    assert!(hosts.is_empty());
}

#[tokio::test]
async fn delete_host_err_with_active_task() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:03".into(), 1_000_000, None)
        .await
        .unwrap();
    // A pending task makes the host busy.
    c.task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();

    let err = core::di::scope(c.clone(), crate::hosts::delete_host(host.id))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("active tasks"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn deploy_creates_deploy_task() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:04".into(), 1_000_000, None)
        .await
        .unwrap();
    let img = c.image_repo.create_image("img".into()).await.unwrap();

    let task = core::di::scope(
        c.clone(),
        crate::hosts::deploy(model::DeployRequest {
            id: host.id,
            image_id: img.id,
        }),
    )
    .await
    .unwrap();

    assert_eq!(task.r#type, model::TaskType::Deploy);
    assert_eq!(task.image_id, Some(img.id));
    assert_eq!(task.hosts.len(), 1);
    assert_eq!(task.hosts[0].host_id, host.id);
}

#[tokio::test]
async fn reboot_creates_task_without_image() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:05".into(), 1_000_000, None)
        .await
        .unwrap();

    core::di::scope(c.clone(), crate::hosts::reboot(vec![host.id]))
        .await
        .unwrap();

    let tasks = c.task_repo.get_all().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_type, TaskType::Reboot);
    assert!(tasks[0].image_id.is_none());
}

// ----------------------------------------------------------------------------
// tasks
// ----------------------------------------------------------------------------

#[tokio::test]
async fn get_all_tasks_empty_then_reflects_creation() {
    let (c, _guard) = container().await;

    let empty = core::di::scope(c.clone(), crate::tasks::get_all_tasks())
        .await
        .unwrap();
    assert!(empty.is_empty());

    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:06".into(), 1_000_000, None)
        .await
        .unwrap();
    c.task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();

    let tasks = core::di::scope(c.clone(), crate::tasks::get_all_tasks())
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].r#type, model::TaskType::Reboot);
}

#[tokio::test]
async fn cancel_task_ok_then_err_when_not_active() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:07".into(), 1_000_000, None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();

    // First cancel succeeds on the pending task.
    core::di::scope(c.clone(), crate::tasks::cancel_task(task.id))
        .await
        .unwrap();

    // Second cancel fails: the task is now cancelled, not pending/running.
    let err = core::di::scope(c.clone(), crate::tasks::cancel_task(task.id))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("pending or running"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn retry_task_err_when_pending_then_ok_when_cancelled() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:08".into(), 1_000_000, None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();

    // A fresh pending task has nothing to retry.
    let err = core::di::scope(c.clone(), crate::tasks::retry_task(task.id))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("failed or cancelled"),
        "unexpected error: {err}"
    );

    // Once cancelled, retry succeeds.
    c.task_repo.cancel(task.id).await.unwrap();
    core::di::scope(c.clone(), crate::tasks::retry_task(task.id))
        .await
        .unwrap();
}

// ----------------------------------------------------------------------------
// images
// ----------------------------------------------------------------------------

#[tokio::test]
async fn get_all_images_empty_then_reflects_creation() {
    let (c, _guard) = container().await;

    let empty = core::di::scope(c.clone(), crate::images::get_all_images())
        .await
        .unwrap();
    assert!(empty.is_empty());

    c.image_repo.create_image("img".into()).await.unwrap();

    let images = core::di::scope(c.clone(), crate::images::get_all_images())
        .await
        .unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].name, "img");
}

#[tokio::test]
async fn create_image_creates_image_and_capture_task() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:09".into(), 1_000_000, None)
        .await
        .unwrap();

    let image = core::di::scope(
        c.clone(),
        crate::images::create_image(model::CreateImageRequest {
            name: "cap".into(),
            host_id: host.id,
        }),
    )
    .await
    .unwrap();
    assert_eq!(image.name, "cap");

    let tasks = c.task_repo.get_all().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_type, TaskType::Capture);
    assert_eq!(tasks[0].image_id, Some(image.id));
}

#[tokio::test]
async fn delete_image_ok_without_tasks_err_with_active_capture() {
    let (c, _guard) = container().await;

    // No tasks: deletion succeeds.
    let free = c.image_repo.create_image("free".into()).await.unwrap();
    core::di::scope(c.clone(), crate::images::delete_image(free.id))
        .await
        .unwrap();

    // Active capture task: deletion is refused.
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:0a".into(), 1_000_000, None)
        .await
        .unwrap();
    let busy = c.image_repo.create_image("busy".into()).await.unwrap();
    c.task_repo
        .create(TaskType::Capture, vec![host.id], Some(busy.id))
        .await
        .unwrap();

    let err = core::di::scope(c.clone(), crate::images::delete_image(busy.id))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("active tasks"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn update_image_name_renames() {
    let (c, _guard) = container().await;
    let img = c.image_repo.create_image("old".into()).await.unwrap();

    let updated = core::di::scope(
        c.clone(),
        crate::images::update_image_name(model::UpdateName {
            id: img.id,
            new_name: "new".into(),
        }),
    )
    .await
    .unwrap();

    assert_eq!(updated.id, img.id);
    assert_eq!(updated.name, "new");
}

// ----------------------------------------------------------------------------
// groups
// ----------------------------------------------------------------------------

#[tokio::test]
async fn groups_full_roundtrip() {
    let (c, _guard) = container().await;
    let h1 = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:0b".into(), 1_000_000, None)
        .await
        .unwrap();
    let h2 = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:0c".into(), 1_000_000, None)
        .await
        .unwrap();

    // create
    let group = core::di::scope(
        c.clone(),
        crate::groups::create_group(model::CreateGroupRequest {
            name: "grp".into(),
            host_ids: vec![h1.id],
        }),
    )
    .await
    .unwrap();
    assert_eq!(group.name, "grp");

    // get_all
    let groups = core::di::scope(c.clone(), crate::groups::get_all_groups())
        .await
        .unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id, group.id);

    // rename
    let renamed = core::di::scope(
        c.clone(),
        crate::groups::update_group_name(model::UpdateName {
            id: group.id,
            new_name: "grp2".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(renamed.name, "grp2");

    // membership update: now both hosts belong to the group
    core::di::scope(
        c.clone(),
        crate::groups::update_group_memberships(model::UpdateGroupRequest {
            id: group.id,
            host_ids: vec![h1.id, h2.id],
        }),
    )
    .await
    .unwrap();
    let members = core::di::scope(c.clone(), crate::hosts::get_hosts_by_group(group.id))
        .await
        .unwrap();
    assert_eq!(members.len(), 2);

    // delete
    core::di::scope(c.clone(), crate::groups::delete_group(group.id))
        .await
        .unwrap();
    let groups = core::di::scope(c.clone(), crate::groups::get_all_groups())
        .await
        .unwrap();
    assert!(groups.is_empty());
}

// ---------------------------------------------------------------------------
// router-level contracts: verbs, paths and error -> status mapping
// ---------------------------------------------------------------------------

fn ui_router() -> axum::Router {
    dioxus::server::ServerFunction::collect()
        .into_iter()
        .filter(|f| f.path().starts_with("/api/ui"))
        .fold(axum::Router::new(), |router, f| {
            router.route(f.path(), f.method_router())
        })
        .with_state(dioxus::server::FullstackState::headless())
}

async fn request(method: Method, uri: &str, body: Option<serde_json::Value>) -> Response {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap())),
        None => builder.body(Body::empty()),
    };
    ui_router().oneshot(request.unwrap()).await.unwrap()
}

async fn get(uri: &str) -> Response {
    request(Method::GET, uri, None).await
}

async fn post(uri: &str, body: serde_json::Value) -> Response {
    request(Method::POST, uri, Some(body)).await
}

async fn json<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn ui_api_contracts() {
    let (c, _guard) = install_container().await;

    reads_are_get_only(&c).await;
    group_members_are_served_from_the_group_path(&c).await;
    deleting_a_busy_host_is_a_bad_request(&c).await;
    cancelling_a_finished_task_is_a_bad_request(&c).await;
    retrying_a_pending_task_is_a_bad_request(&c).await;
    deleting_a_capturing_image_is_a_bad_request(&c).await;
    multicast_creates_a_task_for_every_host(&c).await;
    waking_hosts_is_accepted(&c).await;
}

async fn reads_are_get_only(c: &DIContainer) {
    c.host_repo
        .upsert_host("aa:bb:cc:dd:ee:10".into(), 1_000_000, None)
        .await
        .unwrap();

    for uri in [
        "/api/ui/hosts",
        "/api/ui/images",
        "/api/ui/groups",
        "/api/ui/tasks",
    ] {
        assert_eq!(get(uri).await.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            post(uri, serde_json::json!({})).await.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{uri} accepted a POST"
        );
    }

    assert_eq!(
        get("/api/ui/hosts/rename").await.status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
}

async fn group_members_are_served_from_the_group_path(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:11".into(), 1_000_000, None)
        .await
        .unwrap();
    let group = c
        .group_repo
        .create_group("members", &[host.id])
        .await
        .unwrap();

    let response = get(&format!("/api/ui/groups/{}/hosts", group.id)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let members: Vec<model::Host> = json(response).await;
    assert_eq!(
        members.iter().map(|h| h.id).collect::<Vec<_>>(),
        vec![host.id]
    );
}

async fn deleting_a_busy_host_is_a_bad_request(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:12".into(), 1_000_000, None)
        .await
        .unwrap();
    c.task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();

    let response = post("/api/ui/hosts/delete", serde_json::json!({ "id": host.id })).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(c.host_repo.get_by_mac("aa:bb:cc:dd:ee:12").await.is_ok());
}

async fn cancelling_a_finished_task_is_a_bad_request(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:13".into(), 1_000_000, None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();
    c.task_repo.mark_finished(task.id, host.id).await.unwrap();

    let response = post("/api/ui/tasks/cancel", serde_json::json!({ "id": task.id })).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

async fn retrying_a_pending_task_is_a_bad_request(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:14".into(), 1_000_000, None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();

    let response = post("/api/ui/tasks/retry", serde_json::json!({ "id": task.id })).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

async fn deleting_a_capturing_image_is_a_bad_request(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:15".into(), 1_000_000, None)
        .await
        .unwrap();
    let image = c.image_repo.create_image("busy".into()).await.unwrap();
    c.task_repo
        .create(TaskType::Capture, vec![host.id], Some(image.id))
        .await
        .unwrap();

    let response = post(
        "/api/ui/images/delete",
        serde_json::json!({ "id": image.id }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(c.image_repo.get_status(image.id).await.is_ok());
}

async fn multicast_creates_a_task_for_every_host(c: &DIContainer) {
    let first = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:16".into(), 1_000_000, None)
        .await
        .unwrap();
    let second = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:17".into(), 1_000_000, None)
        .await
        .unwrap();
    let image = c.image_repo.create_image("cast".into()).await.unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();

    let response = post(
        "/api/ui/groups/multicast",
        serde_json::json!({ "req": { "host_ids": [first.id, second.id], "image_id": image.id } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let task = c.task_repo.get_next(first.id).await.unwrap().unwrap();
    assert_eq!(task.task_type, TaskType::Multicast);
    assert_eq!(task.image_id, Some(image.id));
    assert_eq!(
        c.task_repo.get_next(second.id).await.unwrap().unwrap().id,
        task.id
    );
}

async fn waking_hosts_is_accepted(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:18".into(), 1_000_000, None)
        .await
        .unwrap();

    let response = post(
        "/api/ui/hosts/wake",
        serde_json::json!({ "host_ids": [host.id] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}
