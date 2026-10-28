use std::time::{Duration, Instant};

use imaged_core::domain::task::TaskType;
use imaged_e2e::harness::{self, seed};
use imaged_model::AgentEvent;
use imaged_model::agent::{MAC_HEADER, route};
use imaged_model::host::HostPartition;
use reqwest::StatusCode;
use serde_json::json;

const DISK: u64 = 6 * 1024 * 1024 * 1024;

fn connected(s: &harness::TestServer, id: i64) -> bool {
    s.container.host_registry.connected_hosts().contains(&id)
}

async fn feed_state(feed: &mut harness::EventStream, id: i64, timeout: Duration) -> Option<bool> {
    let deadline = Instant::now() + timeout;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let event = feed.next_event(remaining).await?;
        if let Some(hosts) = event.get("Connected").and_then(|v| v.as_array()) {
            return Some(hosts.iter().any(|h| h.as_i64() == Some(id)));
        }
        if let Some(change) = event.get("Changed")
            && change.get("id").and_then(|v| v.as_i64()) == Some(id)
        {
            return change.get("connected").and_then(|v| v.as_bool());
        }
    }
    None
}

async fn poll_until(mut pred: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if pred() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn connecting_upserts_the_host_and_marks_it_connected() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");

    let host = s
        .container
        .host_repo
        .get_by_mac(&mac)
        .await
        .expect("host upserted by connecting");
    assert_eq!(host.mac_address, mac);
    assert_eq!(host.disk, Ok(DISK));

    assert!(
        poll_until(|| connected(s, host.id), Duration::from_secs(1)).await,
        "host should be marked connected in the registry"
    );

    drop(stream);
}

#[tokio::test]
async fn a_pending_task_is_delivered_as_the_first_event() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, DISK).await;
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("pending-img"),
        b"pt",
        &[(1, "ext4", &[1u8; 2048])],
    )
    .await;
    let task_id = seed::deploy_task(s, host.id, image_id).await;

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let event = stream
        .next_event(Duration::from_secs(5))
        .await
        .expect("first event is the pending task");
    assert_eq!(event["Current"]["id"], task_id);
    assert_eq!(event["Current"]["work"]["Deploy"]["image_id"], image_id);
}

#[tokio::test]
async fn a_host_without_a_pending_task_is_told_it_has_none() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let event = stream.next_event(Duration::from_secs(5)).await;
    assert_eq!(event, Some(json!({ "Current": null })));
    let event = stream.next_event(Duration::from_millis(500)).await;
    assert!(event.is_none(), "nothing changed, yet got {event:?}");
}

#[tokio::test]
async fn a_dispatch_sends_the_new_current_task_only_when_it_changed() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    assert_eq!(
        stream.next_event(Duration::from_secs(5)).await,
        Some(json!({ "Current": null }))
    );
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("dispatch-img"),
        b"pt",
        &[(1, "ext4", &[3u8; 512])],
    )
    .await;
    let task_id = seed::deploy_task(s, host.id, image_id).await;

    s.container.host_registry.dispatch(host.id);
    let event = stream
        .next_event(Duration::from_secs(2))
        .await
        .expect("the queued task arrives on the open stream");
    assert_eq!(event["Current"]["id"], task_id);
    assert_eq!(event["Current"]["work"]["Deploy"]["image_id"], image_id);

    s.container.host_registry.dispatch(host.id);
    let event = stream.next_event(Duration::from_millis(500)).await;
    assert!(
        event.is_none(),
        "the current task did not change, yet got {event:?}"
    );

    s.on_server(async move { s.container.task_repo.cancel(task_id).await })
        .await
        .expect("cancel");
    s.container.host_registry.dispatch(host.id);
    assert_eq!(
        stream.next_event(Duration::from_secs(2)).await,
        Some(json!({ "Current": null })),
        "a cancelled task is no longer current"
    );
}

#[tokio::test]
async fn a_second_stream_displaces_the_first_and_keeps_the_host_connected() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let mut first = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    assert_eq!(
        first.next_event(Duration::from_secs(5)).await,
        Some(json!({ "Current": null }))
    );
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    let mut second = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("a second stream for the same host displaces the first");

    assert!(
        first.next_event(Duration::from_secs(2)).await.is_none(),
        "the displaced first connection is closed by the server"
    );
    assert!(
        connected(s, host.id),
        "the host stays connected across the displacement"
    );

    assert_eq!(
        second.next_event(Duration::from_secs(2)).await,
        Some(json!({ "Current": null }))
    );
    let task_id = s
        .on_server(async move {
            s.container
                .task_repo
                .create(TaskType::Reboot, vec![host.id], None)
                .await
        })
        .await
        .expect("reboot task")
        .id;
    s.container.host_registry.dispatch(host.id);
    let event = second
        .next_event(Duration::from_secs(2))
        .await
        .expect("events after displacement are delivered to the new connection");
    assert_eq!(event["Current"]["id"], task_id);

    assert!(connected(s, host.id));
}

#[tokio::test]
async fn dropping_the_stream_deregisters_promptly() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    let dropped_at = Instant::now();
    drop(stream);

    let mut latency = None;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !connected(s, host.id) {
            latency = Some(dropped_at.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let latency = latency.expect("a clean close deregisters the host without an induced write");
    eprintln!("clean-close deregistration observed in {latency:?}");
}

#[tokio::test]
async fn disconnect_deregisters_and_repeats_are_idempotent() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    let resp = s.agent_post_empty(route::DISCONNECT, &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(poll_until(|| !connected(s, host.id), Duration::from_secs(1)).await);

    let resp = s.agent_post_empty(route::DISCONNECT, &mac).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a second disconnect for a known host is a no-op"
    );

    let unknown = harness::unique_mac();
    let resp = s.agent_post_empty(route::DISCONNECT, &unknown).await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "disconnecting an unknown mac reports the missing host"
    );

    drop(stream);
}

#[tokio::test]
async fn a_host_can_reconnect_after_disconnecting() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let first = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();

    let resp = s.agent_post_empty(route::DISCONNECT, &mac).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(poll_until(|| !connected(s, host.id), Duration::from_secs(1)).await);
    drop(first);

    let mut second = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("reconnect");
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        connected(s, host.id),
        "the reconnected registration must survive the old one's late cleanup"
    );

    assert_eq!(
        second.next_event(Duration::from_secs(2)).await,
        Some(json!({ "Current": null }))
    );
    let task_id = s
        .on_server(async move {
            s.container
                .task_repo
                .create(TaskType::Reboot, vec![host.id], None)
                .await
        })
        .await
        .expect("reboot task")
        .id;
    s.container.host_registry.dispatch(host.id);
    let event = second
        .next_event(Duration::from_secs(2))
        .await
        .expect("reconnected stream receives events");
    assert_eq!(event["Current"]["id"], task_id);
}

#[tokio::test]
async fn a_missing_mac_header_is_rejected() {
    let s = harness::server().await;
    let url = format!("{}{}?disk_size_bytes={DISK}", s.base_url, route::STREAM);
    let resp = s.http.get(url).send().await.expect("request");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_non_ascii_mac_header_is_rejected() {
    let s = harness::server().await;
    let url = format!("{}{}?disk_size_bytes={DISK}", s.base_url, route::STREAM);
    let value = reqwest::header::HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap();
    let resp = s
        .http
        .get(url)
        .header(MAC_HEADER, value)
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_malformed_mac_is_refused_and_any_spelling_of_a_mac_names_one_host() {
    let s = harness::server().await;
    let junk = format!("not-a-mac-{}", harness::unique_name("junk"));
    assert!(
        harness::connect_agent_stream(s, &junk, 4096).await.is_err(),
        "a header that is not a MAC address registers no host"
    );

    let mac = harness::unique_mac();
    let shouted = mac.to_ascii_uppercase().replace(':', "-");
    let stream = harness::connect_agent_stream(s, &shouted, 4096)
        .await
        .expect("an upper-case, dash-separated MAC is accepted");
    let host = s
        .container
        .host_repo
        .get_by_mac(&mac)
        .await
        .expect("the host is stored under the canonical MAC");
    assert_eq!(host.mac_address, mac);
    assert_eq!(host.disk, Ok(4096));

    drop(stream);
}

#[tokio::test]
async fn an_unresponsive_agent_is_evicted_by_the_liveness_deadline() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let held = harness::connect_silent_agent(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    let connected_at = Instant::now();
    let mut latency = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !connected(s, host.id) {
            latency = Some(connected_at.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let latency = latency.expect("an agent that stops answering pings is evicted");
    eprintln!("liveness eviction observed in {latency:?}");

    drop(held);
}

#[tokio::test]
async fn a_responsive_agent_is_not_evicted() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    // Polling the socket lets tungstenite auto-pong the server's pings; held
    // open well past the liveness timeout, the host must never be evicted.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let _ = stream.next_event(Duration::from_millis(100)).await;
        assert!(
            connected(s, host.id),
            "a responsive agent must not be evicted"
        );
    }

    drop(stream);
}

#[tokio::test]
async fn the_dashboard_feed_opens_with_the_hosts_that_are_already_connected() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let _agent = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    let mut feed = harness::connect_connection_state(s)
        .await
        .expect("dashboard feed");
    assert_eq!(
        feed_state(&mut feed, host.id, Duration::from_secs(2)).await,
        Some(true),
        "the feed must open with a snapshot naming the connected host"
    );
}

#[tokio::test]
async fn a_reconnect_never_reports_the_host_as_disconnected_on_the_dashboard_feed() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let first = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    let mut feed = harness::connect_connection_state(s)
        .await
        .expect("dashboard feed");
    assert_eq!(
        feed_state(&mut feed, host.id, Duration::from_secs(2)).await,
        Some(true)
    );

    let _second = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("a reconnect overrides the previous connection");
    assert_eq!(
        feed_state(&mut feed, host.id, Duration::from_secs(2)).await,
        Some(true),
        "the displacing connection must be reported as connected"
    );

    drop(first);
    assert_ne!(
        feed_state(&mut feed, host.id, Duration::from_secs(1)).await,
        Some(false),
        "tearing down the displaced connection must not disconnect the host"
    );
    assert!(connected(s, host.id));
}

#[tokio::test]
async fn a_reported_partition_inventory_replaces_the_hosts_previous_one() {
    let s = harness::server().await;
    let mac = harness::unique_mac();
    let partition = |number: i64, partuuid: &str| HostPartition {
        number,
        partuuid: partuuid.to_string(),
        partlabel: Some(format!("part{number}")),
        label: None,
        uuid: Some(format!("uuid-{number}")),
        fstype: Some("ext4".to_string()),
        size_bytes: 1 << 30,
    };

    let mut agent = harness::connect_silent_agent(s, &mac, DISK)
        .await
        .expect("connect");
    agent
        .send(&AgentEvent::Partitions(vec![
            partition(1, "esp"),
            partition(2, "root"),
        ]))
        .await
        .expect("send the first inventory");
    agent
        .send(&AgentEvent::Partitions(vec![partition(2, "root")]))
        .await
        .expect("send the second inventory");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
        if host.partitions == [partition(2, "root")] {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the inventory was not replaced: {:?}",
            host.partitions
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
