use std::time::Duration;

use reqwest::StatusCode;
use serde_json::json;

use imaged_core::domain::task::TaskType;
use imaged_e2e::harness::{self, seed};

const DISK: u64 = 8 * 1024 * 1024 * 1024;

async fn empty_image(s: &harness::TestServer) -> i64 {
    s.container
        .image_repo
        .create_image(harness::unique_name("mc-img"))
        .await
        .expect("create image")
        .id
}

fn host_state(
    task: &imaged_core::domain::task::Task,
    host_id: i64,
) -> imaged_core::domain::task::TaskState {
    task.hosts
        .iter()
        .find(|h| h.host_id == host_id)
        .expect("host row")
        .state
}

async fn get_task(s: &harness::TestServer, id: i64) -> imaged_core::domain::task::Task {
    s.container.task_repo.get(id).await.expect("task")
}

#[tokio::test]
async fn reboot_streams_a_reboot_event_and_completes() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect stream");

    let resp = s
        .ui_post("/api/ui/hosts/reboot", &json!({ "host_ids": [host.id] }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let event = stream
        .next_event(Duration::from_secs(5))
        .await
        .expect("reboot event");
    assert_eq!(event["Task"]["task_type"], "Reboot");
    assert!(event["Task"]["image_id"].is_null());
    let task_id = event["Task"]["id"].as_i64().expect("task id");

    let resp = s
        .agent_post_empty(&format!("/api/client/tasks/{task_id}/finished"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(get_task(s, task_id).await.aggregate_state().is_done());
}

#[tokio::test]
async fn a_reboot_task_for_an_unconnected_host_is_persisted() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;

    let resp = s
        .ui_post("/api/ui/hosts/reboot", &json!({ "host_ids": [host.id] }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let pending = s
        .container
        .task_repo
        .get_next(host.id)
        .await
        .expect("get_next")
        .expect("a persisted reboot task");
    assert_eq!(pending.task_type, TaskType::Reboot);
    assert_eq!(pending.image_id, None);
}

// The `/api/ui/groups/multicast` HTTP route calls `multicast_mgr.notify_new`,
// which detaches a `do_work` that starts every host row and then blocks in a
// real `udp-sender` on the immortal server runtime. Driving it over HTTP both
// leaks a blocked sender and races the pending-host assertion, so this test
// exercises the deterministic, synchronous half of the route (task creation +
// registry fan-out) directly and leaves `notify_new` alone.

#[tokio::test]
async fn multicast_creates_one_task_and_notifies_every_connected_agent() {
    let s = harness::server().await;

    let mac1 = harness::unique_mac();
    let h1 = seed::host(s, &mac1, DISK).await;
    let mac2 = harness::unique_mac();
    let h2 = seed::host(s, &mac2, DISK).await;
    let mac3 = harness::unique_mac();
    let h3 = seed::host(s, &mac3, DISK).await;

    let resp = s
        .ui_post(
            "/api/ui/groups/create",
            &json!({ "req": {
                "name": harness::unique_name("cast-grp"),
                "host_ids": [h1.id, h2.id, h3.id],
            }}),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let image_id = empty_image(s).await;

    let mut s1 = harness::connect_agent_stream(s, &mac1, DISK)
        .await
        .expect("stream h1");
    let mut s2 = harness::connect_agent_stream(s, &mac2, DISK)
        .await
        .expect("stream h2");

    let task = s
        .container
        .task_repo
        .create(TaskType::Multicast, vec![h1.id, h2.id, h3.id], Some(image_id))
        .await
        .expect("create multicast task");
    for id in [h1.id, h2.id, h3.id] {
        s.container.host_registry.send_task(id, &task);
    }

    assert_eq!(task.task_type, TaskType::Multicast);
    let mut covered: Vec<i64> = task.hosts.iter().map(|h| h.host_id).collect();
    covered.sort_unstable();
    let mut expected = vec![h1.id, h2.id, h3.id];
    expected.sort_unstable();
    assert_eq!(covered, expected);

    let e1 = s1.next_event(Duration::from_secs(5)).await.expect("h1 event");
    assert_eq!(e1["Task"]["id"], task.id);
    assert_eq!(e1["Task"]["task_type"], "Multicast");
    assert_eq!(e1["Task"]["image_id"], image_id);

    let e2 = s2.next_event(Duration::from_secs(5)).await.expect("h2 event");
    assert_eq!(e2["Task"]["id"], task.id);
    assert_eq!(e2["Task"]["task_type"], "Multicast");

    let next = s
        .container
        .task_repo
        .get_next_multicast()
        .await
        .expect("get_next_multicast")
        .expect("a pending multicast task");
    assert_eq!(next.id, task.id);
    assert!(next.hosts.iter().any(|h| h.state.is_pending()));
}

#[tokio::test]
async fn cancelling_propagates_to_running_hosts_but_not_to_a_done_host() {
    let s = harness::server().await;

    let mac1 = harness::unique_mac();
    let h1 = seed::host(s, &mac1, DISK).await;
    let mac2 = harness::unique_mac();
    let h2 = seed::host(s, &mac2, DISK).await;
    let image_id = empty_image(s).await;

    let task = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![h1.id, h2.id], Some(image_id))
        .await
        .expect("create task");
    s.container.task_repo.start(task.id, h1.id).await.unwrap();
    s.container
        .task_repo
        .mark_finished(task.id, h2.id)
        .await
        .unwrap();

    let mut s1 = harness::connect_agent_stream(s, &mac1, DISK)
        .await
        .expect("stream h1");
    let mut s2 = harness::connect_agent_stream(s, &mac2, DISK)
        .await
        .expect("stream h2");

    let init = s1
        .next_event(Duration::from_secs(5))
        .await
        .expect("h1 initial running task");
    assert_eq!(init["Task"]["id"], task.id);
    assert!(
        s2.next_event(Duration::from_millis(300)).await.is_none(),
        "a done host has no next task to stream"
    );

    let resp = s
        .ui_post("/api/ui/tasks/cancel", &json!({ "id": task.id }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let ev = s1
        .next_event(Duration::from_secs(5))
        .await
        .expect("h1 cancel event");
    assert_eq!(ev["Cancel"], task.id);
    assert!(
        s2.next_event(Duration::from_millis(300)).await.is_none(),
        "a done host is not sent a cancel event"
    );

    let after = get_task(s, task.id).await;
    assert!(host_state(&after, h1.id).is_cancelled());
    assert!(host_state(&after, h2.id).is_done());
}

#[tokio::test]
async fn cancelling_a_terminal_task_is_rejected_but_a_pending_task_is_cancelled() {
    let s = harness::server().await;

    let mac_a = harness::unique_mac();
    let host_a = seed::host(s, &mac_a, DISK).await;
    let image_id = empty_image(s).await;

    let terminal = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![host_a.id], Some(image_id))
        .await
        .unwrap();
    s.container
        .task_repo
        .mark_finished(terminal.id, host_a.id)
        .await
        .unwrap();
    let resp = s
        .ui_post("/api/ui/tasks/cancel", &json!({ "id": terminal.id }))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let mac_b = harness::unique_mac();
    let host_b = seed::host(s, &mac_b, DISK).await;
    let pending = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![host_b.id], Some(image_id))
        .await
        .unwrap();
    let resp = s
        .ui_post("/api/ui/tasks/cancel", &json!({ "id": pending.id }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(get_task(s, pending.id).await.aggregate_state().is_cancelled());
}

#[tokio::test]
async fn retrying_a_task_whose_image_is_soft_deleted_is_rejected() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = empty_image(s).await;

    let task = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![host.id], Some(image_id))
        .await
        .unwrap();
    s.container
        .task_repo
        .mark_all_failed(task.id, "boom")
        .await
        .unwrap();
    s.container.image_repo.delete_image(image_id).await.unwrap();

    let resp = s
        .ui_post("/api/ui/tasks/retry", &json!({ "id": task.id }))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn retrying_a_partial_task_resets_only_the_failed_rows() {
    let s = harness::server().await;

    let mac1 = harness::unique_mac();
    let h1 = seed::host(s, &mac1, DISK).await;
    let mac2 = harness::unique_mac();
    let h2 = seed::host(s, &mac2, DISK).await;
    let image_id = empty_image(s).await;

    let task = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![h1.id, h2.id], Some(image_id))
        .await
        .unwrap();
    s.container
        .task_repo
        .mark_finished(task.id, h1.id)
        .await
        .unwrap();
    s.container
        .task_repo
        .mark_failed(task.id, h2.id, "boom")
        .await
        .unwrap();
    assert!(get_task(s, task.id).await.aggregate_state().is_partial());

    let resp = s
        .ui_post("/api/ui/tasks/retry", &json!({ "id": task.id }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let after = get_task(s, task.id).await;
    assert!(host_state(&after, h1.id).is_done());
    assert!(host_state(&after, h2.id).is_pending());
}

#[tokio::test]
async fn a_retried_task_reaches_a_host_only_when_it_is_that_hosts_next_task() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = empty_image(s).await;

    let older = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![host.id], Some(image_id))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let newer = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![host.id], Some(image_id))
        .await
        .unwrap();
    s.container
        .task_repo
        .mark_all_failed(newer.id, "boom")
        .await
        .unwrap();

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("stream");
    let init = stream
        .next_event(Duration::from_secs(5))
        .await
        .expect("older task streamed first");
    assert_eq!(init["Task"]["id"], older.id);

    let resp = s
        .ui_post("/api/ui/tasks/retry", &json!({ "id": newer.id }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(
        stream.next_event(Duration::from_millis(400)).await.is_none(),
        "the retried task is not this host's next task, so no event is sent"
    );
    assert!(host_state(&get_task(s, newer.id).await, host.id).is_pending());
}

#[tokio::test]
async fn deleting_a_host_with_an_active_task_is_rejected() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = empty_image(s).await;
    s.container
        .task_repo
        .create(TaskType::Deploy, vec![host.id], Some(image_id))
        .await
        .unwrap();

    let resp = s
        .ui_post("/api/ui/hosts/delete", &json!({ "id": host.id }))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(s.container.host_repo.get_by_mac(&mac).await.is_ok());
}

#[tokio::test]
async fn deleting_a_host_with_only_terminal_tasks_cascades_its_task_rows() {
    let s = harness::server().await;

    let mac1 = harness::unique_mac();
    let h1 = seed::host(s, &mac1, DISK).await;
    let mac2 = harness::unique_mac();
    let h2 = seed::host(s, &mac2, DISK).await;
    let image_id = empty_image(s).await;

    let task = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![h1.id, h2.id], Some(image_id))
        .await
        .unwrap();
    s.container
        .task_repo
        .mark_finished(task.id, h1.id)
        .await
        .unwrap();
    s.container
        .task_repo
        .mark_finished(task.id, h2.id)
        .await
        .unwrap();

    let resp = s
        .ui_post("/api/ui/hosts/delete", &json!({ "id": h1.id }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let after = get_task(s, task.id).await;
    assert_eq!(after.hosts.len(), 1);
    assert_eq!(after.hosts[0].host_id, h2.id);
    assert!(after.aggregate_state().is_done());
}

#[tokio::test]
async fn deleting_an_image_cancels_its_active_task() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = empty_image(s).await;
    let task = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![host.id], Some(image_id))
        .await
        .unwrap();

    let resp = s
        .ui_post("/api/ui/images/delete", &json!({ "id": image_id }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(get_task(s, task.id).await.aggregate_state().is_cancelled());
    assert!(
        s.container
            .image_repo
            .get_all()
            .await
            .unwrap()
            .iter()
            .all(|i| i.id != image_id),
        "deleted image should not be listed"
    );
}

#[tokio::test]
async fn deleting_an_image_should_cancel_its_referencing_multicast_task() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("cast-img"),
        b"table",
        &[(1, "ext4", &[0xCD; 128])],
    )
    .await;

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("stream");

    let task = s
        .container
        .task_repo
        .create(TaskType::Multicast, vec![host.id], Some(image_id))
        .await
        .unwrap();

    let resp = s
        .ui_post("/api/ui/images/delete", &json!({ "id": image_id }))
        .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(get_task(s, task.id).await.aggregate_state().is_cancelled());
    let ev = stream
        .next_event(Duration::from_secs(5))
        .await
        .expect("cancel event for the referenced task");
    assert_eq!(ev["Cancel"], task.id);
}
