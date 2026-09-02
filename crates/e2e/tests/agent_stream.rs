use std::time::{Duration, Instant};

use imaged_e2e::harness::{self, seed};
use reqwest::StatusCode;

const DISK: u64 = 6 * 1024 * 1024 * 1024;

fn connected(s: &harness::TestServer, id: i64) -> bool {
    s.container
        .host_registry
        .get_current_state()
        .iter()
        .any(|e| e.id == id && e.connected)
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
    assert_eq!(host.disk_size, DISK);

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
    assert_eq!(event["Task"]["id"], task_id);
    assert_eq!(event["Task"]["task_type"], "Deploy");
    assert_eq!(event["Task"]["image_id"], image_id);
}

#[tokio::test]
async fn a_host_without_a_pending_task_receives_no_task_event() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let event = stream.next_event(Duration::from_millis(500)).await;
    assert!(
        event.is_none(),
        "an idle host must not receive a task event, got {event:?}"
    );
}

#[tokio::test]
async fn live_events_fan_out_to_an_open_stream_in_order() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let mut stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();

    let other = seed::host(s, &harness::unique_mac(), DISK).await;
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("fanout-img"),
        b"pt",
        &[(1, "ext4", &[3u8; 512])],
    )
    .await;
    let task_id = seed::deploy_task(s, other.id, image_id).await;
    let task = s.container.task_repo.get(task_id).await.unwrap();

    s.container.host_registry.send_task(host.id, &task);
    let event = stream
        .next_event(Duration::from_secs(2))
        .await
        .expect("send_task arrives on the open stream");
    assert_eq!(event["Task"]["id"], task_id);
    assert_eq!(event["Task"]["task_type"], "Deploy");
    assert_eq!(event["Task"]["image_id"], image_id);

    s.container.host_registry.cancel_task(host.id, 4242);
    let event = stream
        .next_event(Duration::from_secs(2))
        .await
        .expect("cancel arrives on the open stream");
    assert_eq!(event["Cancel"], 4242);

    for id in [7001, 7002, 7003] {
        s.container.host_registry.cancel_task(host.id, id);
    }
    for id in [7001, 7002, 7003] {
        let event = stream
            .next_event(Duration::from_secs(2))
            .await
            .expect("ordered cancel");
        assert_eq!(event["Cancel"], id);
    }
}

#[tokio::test]
async fn a_second_stream_displaces_the_first_and_keeps_the_host_connected() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let mut first = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
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

    s.container.host_registry.cancel_task(host.id, 999);
    let event = second
        .next_event(Duration::from_secs(2))
        .await
        .expect("events after displacement are delivered to the new connection");
    assert_eq!(event["Cancel"], 999);

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
    let latency =
        latency.expect("a clean close deregisters the host without an induced write");
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

    let resp = s
        .agent_post_empty("/api/client/stream/disconnect", &mac)
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(poll_until(|| !connected(s, host.id), Duration::from_secs(1)).await);

    let resp = s
        .agent_post_empty("/api/client/stream/disconnect", &mac)
        .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a second disconnect for a known host is a no-op"
    );

    let unknown = harness::unique_mac();
    let resp = s
        .agent_post_empty("/api/client/stream/disconnect", &unknown)
        .await;
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

    let resp = s
        .agent_post_empty("/api/client/stream/disconnect", &mac)
        .await;
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

    s.container.host_registry.cancel_task(host.id, 555);
    let event = second
        .next_event(Duration::from_secs(2))
        .await
        .expect("reconnected stream receives events");
    assert_eq!(event["Cancel"], 555);
}

#[tokio::test]
async fn a_missing_mac_header_is_rejected() {
    let s = harness::server().await;
    let url = format!("{}/api/client/stream?disk_size_bytes={DISK}", s.base_url);
    let resp = s.http.get(url).send().await.expect("request");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_non_ascii_mac_header_is_rejected() {
    let s = harness::server().await;
    let url = format!("{}/api/client/stream?disk_size_bytes={DISK}", s.base_url);
    let value = reqwest::header::HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap();
    let resp = s
        .http
        .get(url)
        .header("X-Agent-Mac", value)
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_ascii_mac_of_any_shape_is_accepted() {
    let s = harness::server().await;
    let junk = format!("not-a-mac-{}", harness::unique_name("junk"));

    let stream = harness::connect_agent_stream(s, &junk, 4096)
        .await
        .expect("an arbitrary ascii mac is accepted; the server does not validate format");
    let host = s
        .container
        .host_repo
        .get_by_mac(&junk)
        .await
        .expect("the arbitrary mac upserted a host");
    assert_eq!(host.disk_size, 4096);

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
    let deadline = Instant::now() + Duration::from_secs(3);
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
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let _ = stream.next_event(Duration::from_millis(100)).await;
        assert!(
            connected(s, host.id),
            "a responsive agent must not be evicted"
        );
    }

    drop(stream);
}
