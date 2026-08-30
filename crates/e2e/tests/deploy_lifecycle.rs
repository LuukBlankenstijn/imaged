use std::time::Duration;

use imaged_core::domain::image::ImageStatus;
use imaged_core::domain::task::{Task, TaskState, TaskType};
use imaged_e2e::harness::{self, seed};
use reqwest::StatusCode;
use serde_json::json;

const DISK: u64 = 8 * 1024 * 1024 * 1024;

fn host_state(task: &Task, host_id: i64) -> TaskState {
    task.hosts
        .iter()
        .find(|h| h.host_id == host_id)
        .expect("host row present on task")
        .state
}

async fn agent_post_json(
    s: &harness::TestServer,
    path: &str,
    mac: &str,
    body: serde_json::Value,
) -> reqwest::Response {
    s.http
        .post(format!("{}{}", s.base_url, path))
        .header("X-Agent-Mac", mac)
        .json(&body)
        .send()
        .await
        .expect("agent_post_json send")
}

async fn ready_image(s: &harness::TestServer, prefix: &str) -> i64 {
    seed::image_with_data(
        s,
        &harness::unique_name(prefix),
        b"parttable",
        &[(1, "ext4", &[5u8; 2048])],
    )
    .await
}

#[tokio::test]
async fn a_deploy_runs_end_to_end_in_the_agents_order() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;

    let parttable: Vec<u8> = (0..300u32).map(|i| (i % 256) as u8).collect();
    let ext4: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    let vfat: Vec<u8> = (0..6144u32).map(|i| ((i * 3 + 1) % 253) as u8).collect();
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("deploy-img"),
        &parttable,
        &[(1, "ext4", ext4.as_slice()), (2, "vfat", vfat.as_slice())],
    )
    .await;
    let task_id = seed::deploy_task(s, host.id, image_id).await;

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let event = stream
        .next_event(Duration::from_secs(5))
        .await
        .expect("task event");
    assert_eq!(event["Task"]["id"], task_id);
    assert_eq!(event["Task"]["task_type"], "Deploy");
    assert_eq!(event["Task"]["image_id"], image_id);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Pending);
    assert_eq!(task.aggregate_state(), TaskState::Pending);

    let resp = s
        .agent_get(&format!("/api/client/tasks/{task_id}/partitions"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let parts: Vec<serde_json::Value> = resp.json().await.expect("partitions json");
    assert_eq!(parts.len(), 2);
    let get = |num: i64| {
        parts
            .iter()
            .find(|p| p["partition_number"].as_i64() == Some(num))
            .expect("partition present")
    };
    assert_eq!(get(1)["fstype"], "ext4");
    assert_eq!(get(1)["size_bytes"].as_u64(), Some(ext4.len() as u64));
    assert_eq!(get(2)["fstype"], "vfat");
    assert_eq!(get(2)["size_bytes"].as_u64(), Some(vfat.len() as u64));

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(
        host_state(&task, host.id),
        TaskState::Pending,
        "listing partitions must not start the task"
    );

    let resp = s
        .agent_get(&format!("/api/client/tasks/{task_id}/parttable"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let table: Vec<u8> = resp.json().await.expect("parttable json");
    assert_eq!(table, parttable);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Running);
    assert_eq!(task.aggregate_state(), TaskState::Running);

    for (n, plain) in [(1i64, &ext4), (2i64, &vfat)] {
        let resp = s
            .agent_get(
                &format!("/api/client/tasks/{task_id}/partitions/{n}/data"),
                &mac,
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let compressed = resp.bytes().await.expect("partition bytes");
        let decompressed = seed::unzstd(&compressed).await;
        assert_eq!(&decompressed, plain);
    }

    let resp = s
        .agent_post_empty(&format!("/api/client/tasks/{task_id}/finished"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Done);
    assert_eq!(task.aggregate_state(), TaskState::Done);

    drop(stream);
}

#[tokio::test]
async fn a_dashboard_deploy_reaches_a_connected_agent() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = ready_image(s, "dash-img").await;

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    assert!(
        stream.next_event(Duration::from_millis(300)).await.is_none(),
        "no task should be queued yet"
    );

    let resp = s
        .ui_post(
            "/api/ui/hosts/deploy",
            &json!({ "req": { "id": host.id, "image_id": image_id } }),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let created: serde_json::Value = resp.json().await.expect("deploy task json");
    let task_id = created["id"].as_i64().expect("task id");

    let event = stream
        .next_event(Duration::from_secs(2))
        .await
        .expect("dashboard deploy pushed to the agent");
    assert_eq!(event["Task"]["id"], task_id);
    assert_eq!(event["Task"]["task_type"], "Deploy");
    assert_eq!(event["Task"]["image_id"], image_id);

    drop(stream);
}

#[tokio::test]
async fn a_failed_deploy_records_the_reason_and_leaves_the_image_alone() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = ready_image(s, "fail-img").await;
    let task_id = seed::deploy_task(s, host.id, image_id).await;

    assert_eq!(
        s.container.image_repo.get_status(image_id).await.unwrap(),
        ImageStatus::Ready
    );

    let reason = "partclone restore failed on the target disk";
    let resp = agent_post_json(
        s,
        &format!("/api/client/tasks/{task_id}/failed"),
        &mac,
        json!({ "error": reason }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Failed);
    assert_eq!(task.aggregate_state(), TaskState::Failed);
    let row = task.hosts.iter().find(|h| h.host_id == host.id).unwrap();
    assert_eq!(row.error.as_deref(), Some(reason));

    assert_eq!(
        s.container.image_repo.get_status(image_id).await.unwrap(),
        ImageStatus::Ready,
        "a failed deploy must not fault the image"
    );
}

#[tokio::test]
async fn a_non_ready_image_is_a_precondition_failure_on_every_download() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image = s
        .container
        .image_repo
        .create_image(harness::unique_name("unready-img"))
        .await
        .unwrap();
    let task_id = seed::deploy_task(s, host.id, image.id).await;

    for path in [
        format!("/api/client/tasks/{task_id}/partitions"),
        format!("/api/client/tasks/{task_id}/parttable"),
        format!("/api/client/tasks/{task_id}/partitions/1/data"),
    ] {
        let resp = s.agent_get(&path, &mac).await;
        assert_eq!(
            resp.status(),
            StatusCode::PRECONDITION_FAILED,
            "{path} against a non-ready image"
        );
    }
}

#[tokio::test]
async fn another_hosts_mac_is_refused_on_every_endpoint() {
    let s = harness::server().await;
    let owner_mac = harness::unique_mac();
    let owner = seed::host(s, &owner_mac, DISK).await;
    let intruder_mac = harness::unique_mac();
    seed::host(s, &intruder_mac, DISK).await;

    let image_id = ready_image(s, "owned-img").await;
    let task_id = seed::deploy_task(s, owner.id, image_id).await;

    for path in [
        format!("/api/client/tasks/{task_id}/partitions"),
        format!("/api/client/tasks/{task_id}/parttable"),
        format!("/api/client/tasks/{task_id}/partitions/1/data"),
    ] {
        let resp = s.agent_get(&path, &intruder_mac).await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{path} served to a non-owner"
        );
    }

    let resp = s
        .agent_post_empty(&format!("/api/client/tasks/{task_id}/finished"), &intruder_mac)
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = agent_post_json(
        s,
        &format!("/api/client/tasks/{task_id}/failed"),
        &intruder_mac,
        json!({ "error": "nope" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_nonexistent_task_id_is_rejected_on_every_endpoint() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = ready_image(s, "real-img").await;
    let task_id = seed::deploy_task(s, host.id, image_id).await;
    let bogus = task_id + 10_000_000;

    for path in [
        format!("/api/client/tasks/{bogus}/partitions"),
        format!("/api/client/tasks/{bogus}/parttable"),
        format!("/api/client/tasks/{bogus}/partitions/1/data"),
    ] {
        let resp = s.agent_get(&path, &mac).await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{path} for a nonexistent task id"
        );
    }

    let resp = s
        .agent_post_empty(&format!("/api/client/tasks/{bogus}/finished"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = agent_post_json(
        s,
        &format!("/api/client/tasks/{bogus}/failed"),
        &mac,
        json!({ "error": "nope" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_nonexistent_partition_number_is_an_internal_error() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = ready_image(s, "part-img").await;
    let task_id = seed::deploy_task(s, host.id, image_id).await;

    let resp = s
        .agent_get(
            &format!("/api/client/tasks/{task_id}/partitions/999/data"),
            &mac,
        )
        .await;
    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "a missing partition file surfaces as 500, not 404"
    );
}

#[tokio::test]
async fn a_multi_host_deploy_goes_partial_then_retries_only_the_failed_host() {
    let s = harness::server().await;
    let mac_a = harness::unique_mac();
    let a = seed::host(s, &mac_a, DISK).await;
    let mac_b = harness::unique_mac();
    let b = seed::host(s, &mac_b, DISK).await;
    let image_id = ready_image(s, "multi-img").await;

    let task_id = s
        .container
        .task_repo
        .create(TaskType::Deploy, vec![a.id, b.id], Some(image_id))
        .await
        .unwrap()
        .id;

    let resp = s
        .agent_post_empty(&format!("/api/client/tasks/{task_id}/finished"), &mac_a)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = agent_post_json(
        s,
        &format!("/api/client/tasks/{task_id}/failed"),
        &mac_b,
        json!({ "error": "disk write error" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, a.id), TaskState::Done);
    assert_eq!(host_state(&task, b.id), TaskState::Failed);
    assert_eq!(task.aggregate_state(), TaskState::Partial);

    let resp = s
        .ui_post("/api/ui/tasks/retry", &json!({ "id": task_id }))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(
        host_state(&task, a.id),
        TaskState::Done,
        "the finished host stays done across a retry"
    );
    assert_eq!(
        host_state(&task, b.id),
        TaskState::Pending,
        "only the failed host is returned to pending"
    );
    assert_eq!(task.aggregate_state(), TaskState::Running);
}

#[tokio::test]
async fn a_large_partition_streams_byte_exact() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;

    let mut big = vec![0u8; 5 * 1024 * 1024];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    for byte in big.iter_mut() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *byte = (state >> 33) as u8;
    }

    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("big-img"),
        b"parttable",
        &[(1, "ext4", big.as_slice())],
    )
    .await;
    let task_id = seed::deploy_task(s, host.id, image_id).await;

    let resp = s
        .agent_get(
            &format!("/api/client/tasks/{task_id}/partitions/1/data"),
            &mac,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let compressed = resp.bytes().await.expect("partition bytes");
    let decompressed = seed::unzstd(&compressed).await;
    assert_eq!(decompressed.len(), big.len());
    assert_eq!(decompressed, big);
}

#[tokio::test]
async fn fetching_the_parttable_twice_keeps_the_task_running() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let parttable: Vec<u8> = (0..200u32).map(|i| (i % 256) as u8).collect();
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("twice-img"),
        &parttable,
        &[(1, "ext4", &[6u8; 1024])],
    )
    .await;
    let task_id = seed::deploy_task(s, host.id, image_id).await;

    let resp = s
        .agent_get(&format!("/api/client/tasks/{task_id}/parttable"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let first: Vec<u8> = resp.json().await.unwrap();
    assert_eq!(first, parttable);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Running);

    let resp = s
        .agent_get(&format!("/api/client/tasks/{task_id}/parttable"), &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let second: Vec<u8> = resp.json().await.unwrap();
    assert_eq!(second, parttable);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Running);
    assert_eq!(task.aggregate_state(), TaskState::Running);
    assert_eq!(
        task.hosts.iter().filter(|h| h.host_id == host.id).count(),
        1,
        "the host must not be duplicated by a second start"
    );
}
