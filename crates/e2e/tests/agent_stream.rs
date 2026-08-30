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

async fn stream_status(s: &harness::TestServer, mac: &str) -> StatusCode {
    let url = format!("{}/api/client/stream?disk_size_bytes={DISK}", s.base_url);
    s.http
        .get(url)
        .header("X-Agent-Mac", mac)
        .send()
        .await
        .expect("stream request")
        .status()
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
async fn a_second_stream_for_the_same_host_is_refused() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let mut first = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();

    assert_eq!(stream_status(s, &mac).await, StatusCode::PRECONDITION_FAILED);

    s.container.host_registry.cancel_task(host.id, 999);
    let event = first
        .next_event(Duration::from_secs(2))
        .await
        .expect("first stream still delivers after the refusal");
    assert_eq!(event["Cancel"], 999);
}

#[tokio::test]
async fn dropping_the_stream_deregisters_after_the_next_write() {
    let s = harness::server().await;
    let mac = harness::unique_mac();

    let stream = harness::connect_agent_stream(s, &mac, DISK)
        .await
        .expect("connect");
    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert!(poll_until(|| connected(s, host.id), Duration::from_secs(1)).await);

    drop(stream);

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut deregistered = false;
    while Instant::now() < deadline {
        s.container.host_registry.cancel_task(host.id, 1);
        tokio::time::sleep(Duration::from_millis(25)).await;
        if !connected(s, host.id) {
            deregistered = true;
            break;
        }
    }
    assert!(
        deregistered,
        "dropping the stream must deregister the host once the server attempts a write"
    );
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
