use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use imaged_core as core;
use tower::ServiceExt as _;

use core::di::DIContainer;
use core::domain::script::NewScript;
use core::domain::task::{MAX_TASK_PAGE_SIZE, Task as DTask, TaskQuery, TaskType};
use core::test_support::container;
use imaged_model::error::AppError;
use imaged_model::image::{ImageKind, ImageStatus};
use imaged_model::script::{InputKind, ScriptInput};
use imaged_model::task::TaskState;
use std::collections::BTreeMap;

use crate::model;

async fn all_tasks(c: &DIContainer) -> Vec<DTask> {
    c.task_repo
        .list(&TaskQuery {
            states: Vec::new(),
            task_types: Vec::new(),
            host_id: None,
            before: None,
            limit: MAX_TASK_PAGE_SIZE,
        })
        .await
        .unwrap()
        .tasks
}

async fn ready_image(c: &DIContainer, name: &str) -> i64 {
    let image = c
        .image_repo
        .create_image(name.into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();
    image.id
}

// ----------------------------------------------------------------------------
// hosts
// ----------------------------------------------------------------------------

#[tokio::test]
async fn update_host_name_renames() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:01".into(), Ok(1_000_000), None)
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
        .upsert_host("aa:bb:cc:dd:ee:02".into(), Ok(1_000_000), None)
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
        .upsert_host("aa:bb:cc:dd:ee:03".into(), Ok(1_000_000), None)
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
        .upsert_host("aa:bb:cc:dd:ee:04".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let image_id = ready_image(&c, "img").await;

    let task = core::di::scope(
        c.clone(),
        crate::hosts::deploy(model::DeployRequest {
            id: host.id,
            image_id,
            inputs: Default::default(),
            partition: None,
        }),
    )
    .await
    .unwrap();

    assert_eq!(task.r#type, model::TaskType::Deploy);
    assert_eq!(task.image_id, Some(image_id));
    assert_eq!(task.hosts.len(), 1);
    assert_eq!(task.hosts[0].host_id, host.id);
    assert_eq!(task.hosts[0].host_name, host.name);
    assert_eq!(task.hosts[0].mac_address, host.mac_address);
}

#[tokio::test]
async fn deploying_or_multicasting_an_image_that_is_not_ready_is_refused_without_a_task() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:4a".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let empty = c
        .image_repo
        .create_image("empty".into(), ImageKind::Disk)
        .await
        .unwrap();
    let faulted = c
        .image_repo
        .create_image("faulted".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_repo.mark_faulted(faulted.id, "boom").await.unwrap();

    for image_id in [empty.id, faulted.id] {
        let deploy = core::di::scope(
            c.clone(),
            crate::hosts::deploy(model::DeployRequest {
                id: host.id,
                image_id,
                inputs: Default::default(),
                partition: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(deploy, AppError::FailedPrecondition(_)),
            "{deploy:?}"
        );

        let multicast = core::di::scope(
            c.clone(),
            crate::groups::multicast(model::MulticastRequest {
                host_ids: vec![host.id],
                image_id,
                inputs: Default::default(),
                partitions: BTreeMap::new(),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(multicast, AppError::FailedPrecondition(_)),
            "{multicast:?}"
        );
    }
    assert!(all_tasks(&c).await.is_empty());
}

#[tokio::test]
async fn a_host_without_a_usable_disk_is_neither_deployed_nor_captured() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host(
            "aa:bb:cc:dd:ee:4b".into(),
            Err("multiple disks found".into()),
            None,
        )
        .await
        .unwrap();
    let image_id = ready_image(&c, "img").await;

    let deploy = core::di::scope(
        c.clone(),
        crate::hosts::deploy(model::DeployRequest {
            id: host.id,
            image_id,
            inputs: Default::default(),
            partition: None,
        }),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&deploy, AppError::FailedPrecondition(m) if m.contains("multiple disks found")),
        "{deploy:?}"
    );

    let capture = core::di::scope(
        c.clone(),
        crate::images::create_image(model::CreateImageRequest {
            name: "cap".into(),
            host_id: host.id,
            partition: None,
        }),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(capture, AppError::FailedPrecondition(_)),
        "{capture:?}"
    );

    assert!(all_tasks(&c).await.is_empty());
    assert_eq!(c.image_repo.get_all().await.unwrap().len(), 1);
}

#[tokio::test]
async fn capturing_from_an_unknown_host_is_not_found_and_leaves_no_image_behind() {
    let (c, _guard) = container().await;

    let err = core::di::scope(
        c.clone(),
        crate::images::create_image(model::CreateImageRequest {
            name: "orphan".into(),
            host_id: 999,
            partition: None,
        }),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, AppError::NotFound(_)), "{err:?}");
    assert!(c.image_repo.get_all().await.unwrap().is_empty());
}

#[tokio::test]
async fn reboot_creates_and_returns_a_task_without_image() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:05".into(), Ok(1_000_000), None)
        .await
        .unwrap();

    let task = core::di::scope(c.clone(), crate::hosts::reboot(vec![host.id]))
        .await
        .unwrap();

    assert_eq!(task.r#type, model::TaskType::Reboot);
    assert!(task.image_id.is_none());
    assert_eq!(
        all_tasks(&c).await.iter().map(|t| t.id).collect::<Vec<_>>(),
        [task.id]
    );
}

// ----------------------------------------------------------------------------
// tasks
// ----------------------------------------------------------------------------

async fn list(c: &DIContainer, query: model::TaskQuery) -> model::TaskPage {
    core::di::scope(c.clone(), crate::tasks::list_tasks(query))
        .await
        .unwrap()
}

fn ids(page: &model::TaskPage) -> Vec<i64> {
    page.tasks.iter().map(|t| t.id).collect()
}

#[tokio::test]
async fn listing_tasks_filters_by_status_type_and_host_and_names_each_host() {
    let (c, _guard) = container().await;
    let a = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:06".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let b = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:4c".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let image = ready_image(&c, "img").await;
    let pending = c
        .task_repo
        .create(TaskType::Reboot, vec![a.id], None)
        .await
        .unwrap();
    let partial = c
        .task_repo
        .create(TaskType::Reboot, vec![a.id, b.id], None)
        .await
        .unwrap();
    c.task_repo.mark_finished(partial.id, a.id).await.unwrap();
    c.task_repo
        .mark_failed(partial.id, b.id, "boom")
        .await
        .unwrap();
    let failed = c
        .task_repo
        .create(TaskType::Deploy, vec![b.id], Some(image))
        .await
        .unwrap();
    c.task_repo
        .mark_failed(failed.id, b.id, "boom")
        .await
        .unwrap();
    let done = c
        .task_repo
        .create(TaskType::Reboot, vec![b.id], None)
        .await
        .unwrap();
    c.task_repo.mark_finished(done.id, b.id).await.unwrap();

    let status = |status| model::TaskQuery {
        status: Some(status),
        ..Default::default()
    };
    let active = list(&c, status(model::TaskStatus::Active)).await;
    assert_eq!(ids(&active), [pending.id]);
    assert_eq!(
        ids(&list(&c, status(model::TaskStatus::Failed)).await),
        [failed.id, partial.id]
    );
    assert_eq!(
        ids(&list(&c, status(model::TaskStatus::Done)).await),
        [done.id]
    );
    assert_eq!(
        ids(&list(
            &c,
            model::TaskQuery {
                task_type: Some(model::TaskType::Deploy),
                ..Default::default()
            }
        )
        .await),
        [failed.id]
    );
    assert_eq!(
        ids(&list(
            &c,
            model::TaskQuery {
                host_id: Some(a.id),
                ..Default::default()
            }
        )
        .await),
        [partial.id, pending.id]
    );

    let host = &active.tasks[0].hosts[0];
    assert_eq!(
        (host.host_name.as_str(), host.mac_address.as_str()),
        (a.name.as_str(), a.mac_address.as_str())
    );
}

#[tokio::test]
async fn task_pages_run_newest_first_and_continue_from_their_cursor() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:4d".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let mut created = Vec::new();
    for _ in 0..3 {
        let task = c
            .task_repo
            .create(TaskType::Reboot, vec![host.id], None)
            .await
            .unwrap();
        created.push(task.id);
    }

    let first = list(
        &c,
        model::TaskQuery {
            limit: Some(2),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(ids(&first), [created[2], created[1]]);

    let second = list(
        &c,
        model::TaskQuery {
            limit: Some(2),
            before: first.next_cursor,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(ids(&second), [created[0]]);
    assert_eq!(second.next_cursor, None);
}

#[tokio::test]
async fn cancel_task_ok_then_err_when_not_active() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:07".into(), Ok(1_000_000), None)
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
        .upsert_host("aa:bb:cc:dd:ee:08".into(), Ok(1_000_000), None)
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

    c.image_repo
        .create_image("img".into(), ImageKind::Disk)
        .await
        .unwrap();

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
        .upsert_host("aa:bb:cc:dd:ee:09".into(), Ok(1_000_000), None)
        .await
        .unwrap();

    let capture = core::di::scope(
        c.clone(),
        crate::images::create_image(model::CreateImageRequest {
            name: "cap".into(),
            host_id: host.id,
            partition: None,
        }),
    )
    .await
    .unwrap();

    assert_eq!(capture.image.name, "cap");
    assert_eq!(capture.task.r#type, model::TaskType::Capture);
    assert_eq!(capture.task.image_id, Some(capture.image.id));
    assert_eq!(
        all_tasks(&c).await.iter().map(|t| t.id).collect::<Vec<_>>(),
        [capture.task.id]
    );
}

#[tokio::test]
async fn deleting_an_image_cancels_its_capture_then_soft_deletes() {
    let (c, guard) = container().await;

    let free = c
        .image_repo
        .create_image("free".into(), ImageKind::Disk)
        .await
        .unwrap();
    core::di::scope(c.clone(), crate::images::delete_image(free.id))
        .await
        .unwrap();

    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:0c".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let img = c
        .image_repo
        .create_image("capturing".into(), ImageKind::Disk)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Capture, vec![host.id], Some(img.id))
        .await
        .unwrap();
    c.image_service
        .save_partition_table(img.id, &[1u8, 2, 3, 4])
        .await
        .unwrap();
    let dir = guard.path().join("images").join(format!("img-{}", img.id));
    assert!(dir.exists());
    let mut conn = c.host_registry.register(host.id);

    core::di::scope(c.clone(), crate::images::delete_image(img.id))
        .await
        .unwrap();

    assert!(
        c.task_repo
            .get(task.id)
            .await
            .unwrap()
            .aggregate_state()
            .is_cancelled(),
        "the capture of a deleted image should be cancelled"
    );
    assert_eq!(
        conn.receiver.try_recv(),
        Ok(()),
        "the capturing host is told"
    );
    assert!(!dir.exists(), "image data directory should be cleared");
    assert!(
        c.image_repo
            .get_all()
            .await
            .unwrap()
            .iter()
            .all(|i| i.id != img.id),
        "soft-deleted image should not be listed"
    );
}

#[tokio::test]
async fn deleting_an_image_is_refused_while_it_is_being_deployed_or_multicast() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:0b".into(), Ok(1_000_000), None)
        .await
        .unwrap();

    for ttype in [TaskType::Deploy, TaskType::Multicast] {
        let image_id = ready_image(&c, &format!("{ttype:?}")).await;
        let task = c
            .task_repo
            .create(ttype, vec![host.id], Some(image_id))
            .await
            .unwrap();

        let err = core::di::scope(c.clone(), crate::images::delete_image(image_id))
            .await
            .unwrap_err();

        assert!(
            matches!(&err, AppError::FailedPrecondition(m) if m.contains(&format!("#{}", task.id))),
            "{err:?}"
        );
        let state = c.task_repo.get(task.id).await.unwrap().aggregate_state();
        assert!(
            state.is_pending() || state.is_running(),
            "{ttype:?} task should be left alone, is {state:?}"
        );
        assert!(
            c.image_repo
                .get_all()
                .await
                .unwrap()
                .iter()
                .any(|i| i.id == image_id),
            "{ttype:?} image should still be listed"
        );
        c.task_repo.cancel(task.id).await.unwrap();
    }
}

#[tokio::test]
async fn update_image_name_renames() {
    let (c, _guard) = container().await;
    let img = c
        .image_repo
        .create_image("old".into(), ImageKind::Disk)
        .await
        .unwrap();

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
        .upsert_host("aa:bb:cc:dd:ee:0b".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let h2 = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:0c".into(), Ok(1_000_000), None)
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
    crate::router().with_state(dioxus::server::FullstackState::headless())
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

async fn upload(uri: &str, body: Body) -> Response {
    let request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/octet-stream")
        .body(body)
        .unwrap();
    ui_router().oneshot(request).await.unwrap()
}

async fn json<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn ui_api_contracts() {
    let (c, _guard) = container().await;
    core::di::init_container(c.clone());

    reads_are_get_only(&c).await;
    group_members_are_served_from_the_group_path(&c).await;
    deleting_a_busy_host_is_a_bad_request(&c).await;
    cancelling_a_finished_task_is_a_bad_request(&c).await;
    retrying_a_pending_task_is_a_bad_request(&c).await;
    deleting_a_capturing_image_cancels_its_task_and_succeeds(&c).await;
    multicast_creates_a_task_for_every_host(&c).await;
    deploy_bodies_reject_unknown_fields(&c).await;
    tasks_are_listed_with_query_filters(&c).await;
    waking_hosts_is_accepted(&c).await;
    images_are_exported_and_imported_as_archives(&c).await;
    scripts_are_managed_and_attached_to_images(&c).await;
}

async fn reads_are_get_only(c: &DIContainer) {
    c.host_repo
        .upsert_host("aa:bb:cc:dd:ee:10".into(), Ok(1_000_000), None)
        .await
        .unwrap();

    for uri in [
        "/api/ui/hosts",
        "/api/ui/images",
        "/api/ui/groups",
        "/api/ui/tasks",
        "/api/ui/scripts",
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
        .upsert_host("aa:bb:cc:dd:ee:11".into(), Ok(1_000_000), None)
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
        .upsert_host("aa:bb:cc:dd:ee:12".into(), Ok(1_000_000), None)
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
        .upsert_host("aa:bb:cc:dd:ee:13".into(), Ok(1_000_000), None)
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
        .upsert_host("aa:bb:cc:dd:ee:14".into(), Ok(1_000_000), None)
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

async fn deleting_a_capturing_image_cancels_its_task_and_succeeds(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:15".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let image = c
        .image_repo
        .create_image("busy".into(), ImageKind::Disk)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Capture, vec![host.id], Some(image.id))
        .await
        .unwrap();

    let response = post(
        "/api/ui/images/delete",
        serde_json::json!({ "id": image.id }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        c.task_repo
            .get(task.id)
            .await
            .unwrap()
            .aggregate_state()
            .is_cancelled(),
        "capture task should be cancelled when its image is deleted"
    );
    assert!(
        c.image_repo
            .get_all()
            .await
            .unwrap()
            .iter()
            .all(|i| i.id != image.id),
        "deleted image should not be listed"
    );
}

async fn multicast_creates_a_task_for_every_host(c: &DIContainer) {
    let first = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:16".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let second = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:17".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let image = c
        .image_repo
        .create_image("cast".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();

    let response = post(
        "/api/ui/groups/multicast",
        serde_json::json!({ "req": { "host_ids": [first.id, second.id], "image_id": image.id } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created: model::Task = json(response).await;

    assert_eq!(created.r#type, model::TaskType::Multicast);
    assert_eq!(created.image_id, Some(image.id));
    let mut host_ids: Vec<i64> = created.hosts.iter().map(|h| h.host_id).collect();
    host_ids.sort();
    assert_eq!(host_ids, [first.id, second.id]);
    assert_eq!(c.task_repo.get(created.id).await.unwrap().hosts.len(), 2);
}

async fn rejected_as_unknown_field(response: Response) -> bool {
    let rejected = !response.status().is_success();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    rejected && String::from_utf8_lossy(&body).contains("unknown field")
}

async fn deploy_bodies_reject_unknown_fields(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:1a".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let image_id = ready_image(c, "strict").await;

    for req in [
        serde_json::json!({ "id": host.id, "image_id": image_id, "input": {} }),
        serde_json::json!({ "id": host.id, "image_id": image_id, "inputs": { "per-machine": {} } }),
    ] {
        let response = post("/api/ui/hosts/deploy", serde_json::json!({ "req": req })).await;
        assert!(rejected_as_unknown_field(response).await, "{req}");
    }
    let response = post(
        "/api/ui/groups/multicast",
        serde_json::json!({ "req": { "host_ids": [host.id], "image_id": image_id, "hosts": [] } }),
    )
    .await;
    assert!(rejected_as_unknown_field(response).await);
    assert!(c.task_repo.get_next(host.id).await.unwrap().is_none());
}

async fn tasks_are_listed_with_query_filters(c: &DIContainer) {
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:1b".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();

    let response = get(&format!(
        "/api/ui/tasks?status=active&type=reboot&host_id={}&limit=5",
        host.id
    ))
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: model::TaskPage = json(response).await;
    assert_eq!(ids(&page), [task.id]);
    assert_eq!(page.next_cursor, None);

    assert!(rejected_as_unknown_field(get("/api/ui/tasks?state=active").await).await);
}

async fn waking_hosts_is_accepted(c: &DIContainer) {
    *c.machine_ip.write() = Some(std::net::Ipv4Addr::LOCALHOST.into());
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:18".into(), Ok(1_000_000), None)
        .await
        .unwrap();

    let response = post(
        "/api/ui/hosts/wake",
        serde_json::json!({ "host_ids": [host.id] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

async fn images_are_exported_and_imported_as_archives(c: &DIContainer) {
    let image = c
        .image_repo
        .create_image("golden image".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_service
        .save_partition_table(image.id, b"table")
        .await
        .unwrap();
    std::fs::write(c.image_service.get_partition_path(image.id, 1), b"data").unwrap();
    c.image_repo
        .save_partition(image.id, 1, "ext4", 4096, None, None)
        .await
        .unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();

    let response = get(&format!("/api/ui/images/{}/export", image.id)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/x-tar");
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"golden_image.imaged\""
    );
    let archive = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    let response = upload("/api/ui/images/import", Body::from(archive)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let imported: model::Image = json(response).await;
    assert_ne!(imported.id, image.id);
    assert_eq!(imported.name, "golden image");
    assert_eq!(imported.status, model::ImageStatus::Ready);
    assert_eq!(imported.partitions.len(), 1);

    let empty = c
        .image_repo
        .create_image("empty".into(), ImageKind::Disk)
        .await
        .unwrap();
    assert_eq!(
        get(&format!("/api/ui/images/{}/export", empty.id))
            .await
            .status(),
        StatusCode::PRECONDITION_FAILED
    );
    assert_eq!(
        upload("/api/ui/images/import", Body::from("not an archive"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let browser_download = Request::builder()
        .uri(format!("/api/ui/images/{}/export", image.id))
        .header("accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .body(Body::empty())
        .unwrap();
    let response = ui_router().oneshot(browser_download).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"golden_image.imaged\""
    );
}

async fn scripts_are_managed_and_attached_to_images(c: &DIContainer) {
    let hostname = serde_json::json!({ "name": "hostname", "kind": "text", "per_machine": true });
    let response = post(
        "/api/ui/scripts/create",
        serde_json::json!({ "req": { "name": "set-hostname", "body": "#!/bin/sh\nhostname", "inputs": [hostname] } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let script: model::Script = json(response).await;
    assert_eq!(script.inputs[0].kind, model::InputKind::Text);
    assert!(script.inputs[0].per_machine);
    assert!(!script.inputs[0].required);
    assert_eq!(script.inputs[0].label, "");

    let invalid = post(
        "/api/ui/scripts/create",
        serde_json::json!({ "req": { "name": "bad name", "body": "x", "inputs": [] } }),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let response = post(
        "/api/ui/scripts/update",
        serde_json::json!({ "req": { "id": script.id, "script": { "name": "hostname", "body": "echo", "inputs": [] } } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: model::Script = json(response).await;
    assert_eq!(
        (updated.name.as_str(), updated.inputs.len()),
        ("hostname", 0)
    );

    let image = c
        .image_repo
        .create_image("scripted".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_repo
        .save_partition(image.id, 2, "ext4", 4096, None, None)
        .await
        .unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();
    let response = post(
        "/api/ui/images/scripts",
        serde_json::json!({ "req": { "image_id": image.id, "script_ids": [script.id] } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let attached: Vec<model::Script> =
        json(get(&format!("/api/ui/images/{}/scripts", image.id)).await).await;
    assert_eq!(
        attached.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![script.id]
    );

    assert_eq!(
        post(
            "/api/ui/scripts/delete",
            serde_json::json!({ "id": script.id })
        )
        .await
        .status(),
        StatusCode::PRECONDITION_FAILED
    );

    assert_eq!(
        post(
            "/api/ui/images/root-partition",
            serde_json::json!({ "req": { "image_id": image.id, "partition_number": 7 } }),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let response = post(
        "/api/ui/images/root-partition",
        serde_json::json!({ "req": { "image_id": image.id, "partition_number": 2 } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let image: model::Image = json(response).await;
    assert_eq!(image.root_partition, Some(2));

    let scripts: Vec<model::Script> = json(get("/api/ui/scripts").await).await;
    assert!(scripts.iter().any(|s| s.id == script.id));

    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:19".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let rejected = post(
        "/api/ui/hosts/deploy",
        serde_json::json!({ "req": { "id": host.id, "image_id": image.id, "inputs": { "shared": { "colour": { "value": "red" } } } } }),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert!(c.task_repo.get_next(host.id).await.unwrap().is_none());

    let response = post(
        "/api/ui/hosts/deploy",
        serde_json::json!({ "req": { "id": host.id, "image_id": image.id } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let task: model::Task = json(response).await;
    c.task_repo
        .start_script_run(task.id, host.id, 0, "hostname")
        .await
        .unwrap();
    c.task_repo
        .append_script_output(task.id, host.id, 0, "set\n")
        .await
        .unwrap();
    let response = get(&format!(
        "/api/ui/tasks/{}/hosts/{}/scripts",
        task.id, host.id
    ))
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let runs: Vec<model::ScriptRun> = json(response).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (
            runs[0].script_name.as_str(),
            runs[0].output.as_str(),
            runs[0].finished_at
        ),
        ("hostname", "set\n", None)
    );
}

#[tokio::test]
async fn cancelling_a_capture_task_marks_the_image_faulted() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:20".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let img = c
        .image_repo
        .create_image("cap".into(), ImageKind::Disk)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Capture, vec![host.id], Some(img.id))
        .await
        .unwrap();

    core::di::scope(c.clone(), crate::tasks::cancel_task(task.id))
        .await
        .unwrap();

    assert_eq!(
        c.image_repo.get_status(img.id).await.unwrap(),
        ImageStatus::Faulted
    );
}

#[tokio::test]
async fn cancelling_a_multicast_task_cancels_its_rows_without_faulting_the_image() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:21".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let img = c
        .image_repo
        .create_image("cast".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_repo.mark_finished(img.id).await.unwrap();
    let task = c
        .task_repo
        .create(TaskType::Multicast, vec![host.id], Some(img.id))
        .await
        .unwrap();

    core::di::scope(c.clone(), crate::tasks::cancel_task(task.id))
        .await
        .unwrap();

    let cancelled = c.task_repo.get(task.id).await.unwrap();
    assert!(cancelled.aggregate_state().is_cancelled());
    assert_eq!(
        c.image_repo.get_status(img.id).await.unwrap(),
        ImageStatus::Ready
    );
}

#[tokio::test]
async fn cancelling_a_task_notifies_only_hosts_with_active_rows() {
    let (c, _guard) = container().await;
    let active = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:22".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let finished = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:23".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![active.id, finished.id], None)
        .await
        .unwrap();
    c.task_repo
        .mark_finished(task.id, finished.id)
        .await
        .unwrap();

    let mut active_conn = c.host_registry.register(active.id);
    let mut finished_conn = c.host_registry.register(finished.id);

    core::di::scope(c.clone(), crate::tasks::cancel_task(task.id))
        .await
        .unwrap();

    assert_eq!(active_conn.receiver.try_recv(), Ok(()));
    assert!(finished_conn.receiver.try_recv().is_err());
}

#[tokio::test]
async fn retrying_a_task_with_no_host_rows_is_rejected() {
    let (c, _guard) = container().await;
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![], None)
        .await
        .unwrap();

    let err = core::di::scope(c.clone(), crate::tasks::retry_task(task.id))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("hosts are deleted"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn retrying_a_task_whose_image_was_deleted_is_rejected() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:24".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let img = c
        .image_repo
        .create_image("doomed".into(), ImageKind::Disk)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Deploy, vec![host.id], Some(img.id))
        .await
        .unwrap();
    c.task_repo.cancel(task.id).await.unwrap();
    c.image_repo.delete_image(img.id).await.unwrap();

    let err = core::di::scope(c.clone(), crate::tasks::retry_task(task.id))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("image is deleted"),
        "unexpected error: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retrying_a_multicast_task_notifies_the_manager_to_pick_it_up() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:25".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let img = c
        .image_repo
        .create_image("cast".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_repo.mark_finished(img.id).await.unwrap();
    let task = c
        .task_repo
        .create(TaskType::Multicast, vec![host.id], Some(img.id))
        .await
        .unwrap();
    c.task_repo.cancel(task.id).await.unwrap();

    core::di::scope(c.clone(), crate::tasks::retry_task(task.id))
        .await
        .unwrap();

    let mut moved = false;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        let current = c.task_repo.get(task.id).await.unwrap();
        if current.hosts[0].state != TaskState::Pending {
            moved = true;
            break;
        }
    }
    assert!(
        moved,
        "manager did not dequeue the retried multicast task off Pending"
    );
}

#[tokio::test]
async fn retrying_a_partial_task_resets_only_the_failed_and_cancelled_host_rows() {
    let (c, _guard) = container().await;
    let done_host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:27".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let failed_host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:28".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![done_host.id, failed_host.id], None)
        .await
        .unwrap();
    c.task_repo
        .mark_finished(task.id, done_host.id)
        .await
        .unwrap();
    c.task_repo
        .mark_failed(task.id, failed_host.id, "boom")
        .await
        .unwrap();

    core::di::scope(c.clone(), crate::tasks::retry_task(task.id))
        .await
        .unwrap();

    let after = c.task_repo.get(task.id).await.unwrap();
    let state_of = |hid: i64| after.hosts.iter().find(|h| h.host_id == hid).unwrap().state;
    assert_eq!(state_of(done_host.id), TaskState::Done);
    assert_eq!(state_of(failed_host.id), TaskState::Pending);
}

#[tokio::test]
async fn deleting_an_image_soft_deletes_the_row_and_clears_its_on_disk_directory() {
    let (c, guard) = container().await;
    let img = c
        .image_repo
        .create_image("gone".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_service
        .save_partition_table(img.id, &[1u8, 2, 3, 4])
        .await
        .unwrap();
    let dir = guard.path().join("images").join(format!("img-{}", img.id));
    assert!(dir.exists());

    core::di::scope(c.clone(), crate::images::delete_image(img.id))
        .await
        .unwrap();

    assert!(!dir.exists(), "image data directory should be cleared");
    assert!(
        c.image_repo
            .get_all()
            .await
            .unwrap()
            .iter()
            .all(|i| i.id != img.id),
        "soft-deleted image should not be listed"
    );
}

#[tokio::test]
async fn deleting_a_host_with_only_terminal_tasks_succeeds_and_cascades_its_task_rows() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:2a".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Reboot, vec![host.id], None)
        .await
        .unwrap();
    c.task_repo.mark_finished(task.id, host.id).await.unwrap();

    core::di::scope(c.clone(), crate::hosts::delete_host(host.id))
        .await
        .unwrap();

    assert!(c.host_repo.get_all(None).await.unwrap().is_empty());
    assert!(
        c.task_repo.get(task.id).await.unwrap().hosts.is_empty(),
        "task_hosts rows should cascade when the host is deleted"
    );
}

#[tokio::test]
async fn deploying_notifies_a_registered_host_and_is_silent_for_an_unregistered_one() {
    let (c, _guard) = container().await;
    let online = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:2b".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let offline = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:2c".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let image_id = ready_image(&c, "os").await;

    let mut conn = c.host_registry.register(online.id);
    core::di::scope(
        c.clone(),
        crate::hosts::deploy(model::DeployRequest {
            id: online.id,
            image_id,
            inputs: Default::default(),
            partition: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(conn.receiver.try_recv(), Ok(()));

    let offline_task = core::di::scope(
        c.clone(),
        crate::hosts::deploy(model::DeployRequest {
            id: offline.id,
            image_id,
            inputs: Default::default(),
            partition: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(offline_task.r#type, model::TaskType::Deploy);
}

#[tokio::test]
async fn waking_hosts_is_refused_while_the_machine_listener_is_not_running() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:2f".into(), Ok(1_000_000), None)
        .await
        .unwrap();

    let err = core::di::scope(c.clone(), crate::hosts::wake_on_lan(vec![host.id]))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("machine listener is not running"),
        "unexpected error: {err}"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn multicast_hands_one_task_to_registered_members_when_its_session_starts() {
    let (c, _guard) = container().await;
    let first = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:30".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let second = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:31".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let absent = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:32".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let img = c
        .image_repo
        .create_image("cast".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_repo.mark_finished(img.id).await.unwrap();

    let mut first_conn = c.host_registry.register(first.id);
    let mut second_conn = c.host_registry.register(second.id);

    core::di::scope(
        c.clone(),
        crate::groups::multicast(model::MulticastRequest {
            host_ids: vec![first.id, second.id, absent.id],
            image_id: img.id,
            inputs: Default::default(),
            partitions: BTreeMap::new(),
        }),
    )
    .await
    .unwrap();

    let task = c.task_repo.get_next(first.id).await.unwrap().unwrap();
    assert_eq!(task.task_type, TaskType::Multicast);
    assert_eq!(
        task.hosts.iter().map(|h| h.host_id).collect::<Vec<_>>(),
        vec![first.id, second.id, absent.id]
    );

    for conn in [&mut first_conn, &mut second_conn] {
        match tokio::time::timeout(Duration::from_secs(5), conn.receiver.recv()).await {
            Ok(Some(())) => {}
            other => panic!("member expected a dispatch, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn connection_state_snapshot_includes_hosts_registered_before_the_call() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:33".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let _conn = c.host_registry.register(host.id);

    assert!(c.host_registry.connected_hosts().contains(&host.id));

    let stream = core::di::scope(c.clone(), crate::realtime::connection_state()).await;
    assert!(stream.is_ok());
}

async fn scripted_image(c: &DIContainer, root: Option<i64>, inputs: Vec<ScriptInput>) -> i64 {
    let image = c
        .image_repo
        .create_image("scripted".into(), ImageKind::Disk)
        .await
        .unwrap();
    c.image_repo
        .save_partition(image.id, 2, "ext4", 4096, None, None)
        .await
        .unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();
    c.image_repo
        .set_root_partition(image.id, root)
        .await
        .unwrap();
    let script = c
        .script_repo
        .create(&NewScript {
            name: "setup".into(),
            body: "echo".into(),
            inputs,
        })
        .await
        .unwrap();
    c.script_repo
        .set_for_image(image.id, &[script.id])
        .await
        .unwrap();
    image.id
}

fn text_input(name: &str, per_machine: bool) -> ScriptInput {
    ScriptInput {
        name: name.into(),
        label: name.into(),
        description: None,
        kind: InputKind::Text,
        required: true,
        default_value: None,
        per_machine,
        choices: Vec::new(),
    }
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn typed(pairs: &[(&str, &str)]) -> model::InputSources {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), model::InputSource::Value(v.to_string())))
        .collect()
}

#[tokio::test]
async fn a_multicast_stores_each_hosts_resolved_input_values_or_is_rejected_without_a_task() {
    let (c, _guard) = container().await;
    let a = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:41".into(), Ok(1_000_000), None)
        .await
        .unwrap()
        .id;
    let b = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:42".into(), Ok(1_000_000), None)
        .await
        .unwrap()
        .id;
    c.host_repo.update_name(b, "lab-b".into()).await.unwrap();
    let mut mode = text_input("mode", false);
    mode.kind = InputKind::Choice;
    mode.choices = vec!["fast".into(), "safe".into()];
    mode.default_value = Some("safe".into());
    let image = scripted_image(
        &c,
        Some(2),
        vec![
            text_input("site", false),
            text_input("hostname", true),
            mode,
        ],
    )
    .await;
    let request = |per_machine: BTreeMap<i64, model::InputSources>| model::MulticastRequest {
        host_ids: vec![a, b],
        image_id: image,
        inputs: model::DeployInputs {
            shared: typed(&[("site", "lab")]),
            per_machine,
        },
        partitions: BTreeMap::new(),
    };

    let err = core::di::scope(
        c.clone(),
        crate::groups::multicast(request(BTreeMap::from([(
            a,
            typed(&[("hostname", "pc-a")]),
        )]))),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, AppError::InvalidArgument(m) if m.contains("lab-b") && m.contains("hostname")),
        "{err:?}"
    );
    assert!(all_tasks(&c).await.is_empty());

    core::di::scope(
        c.clone(),
        crate::groups::multicast(request(BTreeMap::from([
            (a, typed(&[("hostname", "pc-a")])),
            (b, typed(&[("hostname", "pc-b")])),
        ]))),
    )
    .await
    .unwrap();

    let task = c.task_repo.get_next(a).await.unwrap().unwrap();
    assert_eq!(
        c.task_repo.get_inputs(task.id, a).await.unwrap(),
        values(&[("hostname", "pc-a"), ("mode", "safe"), ("site", "lab")])
    );
    assert_eq!(
        c.task_repo.get_inputs(task.id, b).await.unwrap(),
        values(&[("hostname", "pc-b"), ("mode", "safe"), ("site", "lab")])
    );
}

#[tokio::test]
async fn an_input_taken_from_a_script_variable_stores_the_variables_value() {
    let (mut c, _guard) = container().await;
    c.script_variables = std::sync::Arc::new(BTreeMap::from([(
        "HOST_KEY".to_string(),
        "line one\nline two\n".to_string(),
    )]));
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:44".into(), Ok(1_000_000), None)
        .await
        .unwrap()
        .id;
    let mut key = text_input("host_key", true);
    key.kind = InputKind::Secret;
    let image = scripted_image(&c, Some(2), vec![key]).await;
    let request = |variable: &str| model::DeployRequest {
        id: host,
        image_id: image,
        inputs: model::DeployInputs {
            shared: BTreeMap::new(),
            per_machine: BTreeMap::from([(
                host,
                BTreeMap::from([(
                    "host_key".to_string(),
                    model::InputSource::Variable(variable.to_string()),
                )]),
            )]),
        },
        partition: None,
    };

    let err = core::di::scope(c.clone(), crate::hosts::deploy(request("NOPE")))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::InvalidArgument(m) if m.contains("`NOPE`")),
        "{err:?}"
    );
    assert!(all_tasks(&c).await.is_empty());

    let task = core::di::scope(c.clone(), crate::hosts::deploy(request("HOST_KEY")))
        .await
        .unwrap();
    assert_eq!(
        c.task_repo.get_inputs(task.id, host).await.unwrap(),
        values(&[("host_key", "line one\nline two\n")])
    );
    assert_eq!(
        core::di::scope(c.clone(), crate::scripts::get_script_variable_names())
            .await
            .unwrap(),
        ["HOST_KEY"]
    );
}

#[tokio::test]
async fn an_image_with_scripts_but_no_root_partition_is_not_deployed() {
    let (c, _guard) = container().await;
    let host = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:43".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let image = scripted_image(&c, None, Vec::new()).await;

    let err = core::di::scope(
        c.clone(),
        crate::hosts::deploy(model::DeployRequest {
            id: host.id,
            image_id: image,
            inputs: Default::default(),
            partition: None,
        }),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, AppError::FailedPrecondition(_)), "{err:?}");
    assert!(c.task_repo.get_next(host.id).await.unwrap().is_none());
}

#[tokio::test]
async fn kicking_a_receiving_host_fails_only_that_host_and_tells_it_to_stop() {
    let (c, _guard) = container().await;
    let kicked = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:40".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let kept = c
        .host_repo
        .upsert_host("aa:bb:cc:dd:ee:41".into(), Ok(1_000_000), None)
        .await
        .unwrap();
    let img = c
        .image_repo
        .create_image("cast".into(), ImageKind::Disk)
        .await
        .unwrap();
    let task = c
        .task_repo
        .create(TaskType::Multicast, vec![kicked.id, kept.id], Some(img.id))
        .await
        .unwrap();
    let kick = |host_id| {
        core::di::scope(
            c.clone(),
            crate::tasks::kick_host(model::KickRequest {
                task_id: task.id,
                host_id,
            }),
        )
    };

    let err = kick(kicked.id).await.unwrap_err();
    assert!(
        err.to_string().contains("not receiving"),
        "unexpected error: {err}"
    );

    c.task_repo.start(task.id, kicked.id).await.unwrap();
    c.task_repo.start(task.id, kept.id).await.unwrap();
    let mut conn = c.host_registry.register(kicked.id);
    kick(kicked.id).await.unwrap();

    let after = c.task_repo.get(task.id).await.unwrap();
    let state = |id: i64| after.hosts.iter().find(|h| h.host_id == id).unwrap().state;
    assert_eq!(state(kicked.id), TaskState::Failed);
    assert_eq!(state(kept.id), TaskState::Running);
    assert_eq!(conn.receiver.try_recv(), Ok(()));
}

const IMAGE_FS_UUID: &str = "1a2b3c4d-0000-4000-8000-00000000abcd";
const IMAGE_FS_LABEL: &str = "root fs";

fn host_partition(number: i64, size_bytes: u64, uuid: Option<&str>) -> model::HostPartition {
    model::HostPartition {
        number,
        partuuid: format!("0000000{number}-part"),
        partlabel: None,
        label: None,
        uuid: uuid.map(str::to_string),
        fstype: Some(if number == 1 { "vfat" } else { "ext4" }.into()),
        size_bytes,
    }
}

fn labelled(mut partition: model::HostPartition, label: &str) -> model::HostPartition {
    partition.label = Some(label.into());
    partition
}

async fn inventoried_host(c: &DIContainer, mac: &str, partitions: &[model::HostPartition]) -> i64 {
    let host = c
        .host_repo
        .upsert_host(mac.into(), Ok(1 << 40), None)
        .await
        .unwrap();
    c.host_repo
        .set_partitions(host.id, partitions)
        .await
        .unwrap();
    host.id
}

async fn partition_image(c: &DIContainer, size_bytes: i64) -> i64 {
    let image = c
        .image_repo
        .create_image("root".into(), ImageKind::Partition)
        .await
        .unwrap();
    c.image_repo
        .save_partition(
            image.id,
            1,
            "ext4",
            size_bytes,
            Some(IMAGE_FS_UUID),
            Some(IMAGE_FS_LABEL),
        )
        .await
        .unwrap();
    c.image_repo.mark_finished(image.id).await.unwrap();
    image.id
}

async fn deploy_into(
    c: &DIContainer,
    host_id: i64,
    image_id: i64,
    partition: Option<&str>,
) -> Result<model::Task, AppError> {
    core::di::scope(
        c.clone(),
        crate::hosts::deploy(model::DeployRequest {
            id: host_id,
            image_id,
            inputs: Default::default(),
            partition: partition.map(str::to_string),
        }),
    )
    .await
}

#[tokio::test]
async fn a_partition_image_deploys_only_into_a_large_enough_partition_without_a_filesystem_twin() {
    let (c, _guard) = container().await;
    let host = inventoried_host(
        &c,
        "aa:bb:cc:dd:ee:70",
        &[
            host_partition(1, 1 << 29, None),
            host_partition(2, 1 << 30, Some(IMAGE_FS_UUID)),
            host_partition(3, 1 << 33, None),
            host_partition(4, 1 << 33, Some(&IMAGE_FS_UUID.to_uppercase())),
        ],
    )
    .await;
    let image = partition_image(&c, 1 << 30).await;
    let disk_image = ready_image(&c, "disk").await;

    let refusal = |result: Result<model::Task, AppError>| match result {
        Err(AppError::InvalidArgument(_)) => "400",
        Err(AppError::FailedPrecondition(_)) => "412",
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(refusal(deploy_into(&c, host, image, None).await), "400");
    assert_eq!(
        refusal(deploy_into(&c, host, disk_image, Some("00000003-part")).await),
        "400"
    );
    assert_eq!(
        refusal(deploy_into(&c, host, image, Some("missing")).await),
        "412"
    );
    assert_eq!(
        refusal(deploy_into(&c, host, image, Some("00000001-part")).await),
        "412"
    );
    match deploy_into(&c, host, image, Some("00000003-part")).await {
        Err(AppError::FailedPrecondition(message)) => {
            assert!(message.contains("00000002-part"), "{message}")
        }
        other => panic!("expected the UUID twin to be refused, got {other:?}"),
    }
    assert!(all_tasks(&c).await.is_empty());

    c.host_repo
        .set_partitions(
            host,
            &[
                labelled(host_partition(2, 1 << 30, None), IMAGE_FS_LABEL),
                host_partition(3, 1 << 33, None),
            ],
        )
        .await
        .unwrap();
    match deploy_into(&c, host, image, Some("00000003-part")).await {
        Err(AppError::FailedPrecondition(message)) => {
            assert!(message.contains("00000002-part"), "{message}");
            assert!(message.contains("labelled `root fs`"), "{message}");
        }
        other => panic!("expected the label twin to be refused, got {other:?}"),
    }
    assert!(all_tasks(&c).await.is_empty());

    c.host_repo
        .set_partitions(
            host,
            &[
                labelled(
                    host_partition(2, 1 << 30, Some(IMAGE_FS_UUID)),
                    IMAGE_FS_LABEL,
                ),
                host_partition(3, 1 << 33, None),
                labelled(
                    host_partition(4, 1 << 33, None),
                    &IMAGE_FS_LABEL.to_uppercase(),
                ),
            ],
        )
        .await
        .unwrap();
    let task = deploy_into(&c, host, image, Some("00000002-PART"))
        .await
        .unwrap();
    assert_eq!(task.r#type, model::TaskType::Deploy);
    assert_eq!(task.hosts[0].partuuid.as_deref(), Some("00000002-part"));
    let stored = c.task_repo.get(task.id).await.unwrap();
    assert_eq!(stored.task_type, TaskType::DeployPartition);

    let page = list(
        &c,
        model::TaskQuery {
            task_type: Some(model::TaskType::Deploy),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(ids(&page), [task.id]);
}

#[tokio::test]
async fn a_partition_multicast_needs_a_target_partition_on_every_host() {
    let (c, _guard) = container().await;
    let a = inventoried_host(&c, "aa:bb:cc:dd:ee:71", &[host_partition(2, 1 << 31, None)]).await;
    let b = inventoried_host(&c, "aa:bb:cc:dd:ee:72", &[host_partition(5, 1 << 31, None)]).await;
    let stranger = inventoried_host(&c, "aa:bb:cc:dd:ee:73", &[]).await;
    let image = partition_image(&c, 1 << 30).await;
    let multicast = |partitions: BTreeMap<i64, String>| {
        core::di::scope(
            c.clone(),
            crate::groups::multicast(model::MulticastRequest {
                host_ids: vec![a, b],
                image_id: image,
                inputs: Default::default(),
                partitions,
            }),
        )
    };

    let missing = multicast(BTreeMap::from([(a, "00000002-part".to_string())]))
        .await
        .unwrap_err();
    assert!(
        matches!(missing, AppError::InvalidArgument(_)),
        "{missing:?}"
    );
    let stray = multicast(BTreeMap::from([
        (a, "00000002-part".to_string()),
        (b, "00000005-part".to_string()),
        (stranger, "00000002-part".to_string()),
    ]))
    .await
    .unwrap_err();
    assert!(matches!(stray, AppError::InvalidArgument(_)), "{stray:?}");
    assert!(all_tasks(&c).await.is_empty());

    let task = multicast(BTreeMap::from([
        (a, "00000002-part".to_string()),
        (b, "00000005-part".to_string()),
    ]))
    .await
    .unwrap();
    assert_eq!(task.r#type, model::TaskType::Multicast);
    let stored = c.task_repo.get(task.id).await.unwrap();
    assert_eq!(stored.task_type, TaskType::MulticastPartition);
    assert_eq!(stored.partuuid(a), Some("00000002-part"));
    assert_eq!(stored.partuuid(b), Some("00000005-part"));
}

#[tokio::test]
async fn capturing_a_partition_needs_an_inventoried_partition_with_a_partition_image_filesystem() {
    let (c, _guard) = container().await;
    let mut swap = host_partition(3, 1 << 30, None);
    swap.fstype = Some("swap".into());
    let mut btrfs = host_partition(4, 1 << 30, None);
    btrfs.fstype = Some("btrfs".into());
    let host = inventoried_host(
        &c,
        "aa:bb:cc:dd:ee:74",
        &[host_partition(2, 1 << 30, Some(IMAGE_FS_UUID)), swap, btrfs],
    )
    .await;
    let capture = |name: &str, partition: &str| {
        core::di::scope(
            c.clone(),
            crate::images::create_image(model::CreateImageRequest {
                name: name.into(),
                host_id: host,
                partition: Some(partition.into()),
            }),
        )
    };

    for refused in ["missing", "00000003-part"] {
        let err = capture("root", refused).await.unwrap_err();
        assert!(matches!(err, AppError::FailedPrecondition(_)), "{err:?}");
    }
    assert!(c.image_repo.get_all().await.unwrap().is_empty());

    for (name, partuuid) in [("root", "00000002-part"), ("data", "00000004-part")] {
        let captured = capture(name, partuuid).await.unwrap();
        assert_eq!(captured.image.kind, model::ImageKind::Partition);
        assert_eq!(captured.task.r#type, model::TaskType::Capture);
        assert_eq!(captured.task.hosts[0].partuuid.as_deref(), Some(partuuid));
        let stored = c.task_repo.get(captured.task.id).await.unwrap();
        assert_eq!(stored.task_type, TaskType::CapturePartition);
    }
}
