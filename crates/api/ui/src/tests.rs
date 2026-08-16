use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use imaged_core as core;

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
