use std::net::{IpAddr, Ipv4Addr};
use std::pin::Pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use reqwest::Url;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use imaged_agent::server::{Server, Uplink};
use imaged_agent::sys::disk::HostDisk;
use imaged_core::domain::host::Host;
use imaged_core::domain::task::TaskType;
use imaged_e2e::harness::{self, seed};
use imaged_model::agent::{MAC_HEADER, MacAddress, route};
use imaged_model::error::AppError;
use imaged_model::{ServerEvent, Task};

const DISK: u64 = 16 * 1024 * 1024 * 1024;
const AGENT_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9));
const WAIT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(300);
const SILENCE_LIMIT: Duration = Duration::from_secs(30);

type Events = Pin<Box<dyn Stream<Item = ServerEvent> + Send>>;

fn agent(s: &harness::TestServer) -> (Server, String) {
    let mac: MacAddress = harness::unique_mac().parse().expect("unique mac parses");
    let base = Url::parse(&s.base_url).expect("harness base url");
    let server = Server::new(base, mac, Some(AGENT_IP)).expect("agent client");
    (server, mac.to_string())
}

fn usable() -> Result<HostDisk, String> {
    Ok(HostDisk {
        size_bytes: DISK,
        partitions: Vec::new(),
    })
}

async fn connect(
    agent: &Server,
    disk: &Result<HostDisk, String>,
    idle: watch::Receiver<()>,
) -> Events {
    let uplink = Uplink {
        progress: watch::channel(None).1,
        idle,
    };
    Box::pin(
        agent
            .connect_events(disk, uplink, SILENCE_LIMIT)
            .await
            .expect("connect events"),
    )
}

async fn next(events: &mut Events) -> ServerEvent {
    tokio::time::timeout(WAIT, events.next())
        .await
        .expect("an event within the timeout")
        .expect("the stream stays open")
}

async fn assert_quiet(events: &mut Events) {
    let event = tokio::time::timeout(QUIET, events.next()).await;
    assert!(event.is_err(), "expected no event, got {event:?}");
}

fn current(task_id: i64) -> ServerEvent {
    ServerEvent::Current(Some(Task::new(task_id, imaged_model::Work::Reboot)))
}

async fn reboot(s: &'static harness::TestServer, host_id: i64) -> i64 {
    s.on_server(async move {
        s.container
            .task_repo
            .create(TaskType::Reboot, vec![host_id], None)
            .await
    })
    .await
    .expect("create reboot task")
    .id
}

async fn registered_host(s: &'static harness::TestServer, mac: &str) -> Host {
    let mac = mac.to_string();
    s.on_server(async move { s.container.host_repo.get_by_mac(&mac).await })
        .await
        .expect("host registered by the event stream")
}

#[tokio::test]
async fn a_task_queued_while_the_agent_is_busy_is_handed_over_once_it_is_idle() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    let host = seed::host(s, &mac, DISK).await;
    let first = reboot(s, host.id).await;
    let (idle, idle_receiver) = watch::channel(());

    let mut events = connect(&agent, &usable(), idle_receiver).await;
    assert_eq!(next(&mut events).await, current(first));

    let queued = reboot(s, host.id).await;
    s.container.host_registry.dispatch(host.id);
    assert_quiet(&mut events).await;

    agent
        .mark_finished(first)
        .await
        .expect("finish the first task");
    assert_quiet(&mut events).await;

    idle.send_replace(());
    assert_eq!(next(&mut events).await, current(queued));
}

#[tokio::test]
async fn an_idle_agent_that_asks_again_is_told_the_same_current_task() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    let host = seed::host(s, &mac, DISK).await;
    let task = reboot(s, host.id).await;
    let (idle, idle_receiver) = watch::channel(());

    let mut events = connect(&agent, &usable(), idle_receiver).await;
    assert_eq!(next(&mut events).await, current(task));

    idle.send_replace(());
    assert_eq!(next(&mut events).await, current(task));
}

#[tokio::test]
async fn a_task_cancelled_while_the_agent_was_away_is_withdrawn_when_it_reconnects() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    let host = seed::host(s, &mac, DISK).await;
    let task = reboot(s, host.id).await;

    let mut events = connect(&agent, &usable(), watch::channel(()).1).await;
    assert_eq!(next(&mut events).await, current(task));
    drop(events);

    s.on_server(async move { s.container.task_repo.cancel(task).await })
        .await
        .expect("cancel");
    s.container.host_registry.dispatch(host.id);

    let mut events = connect(&agent, &usable(), watch::channel(()).1).await;
    assert_eq!(next(&mut events).await, ServerEvent::Current(None));
}

#[tokio::test]
async fn a_machine_without_a_usable_disk_registers_with_the_reason_and_cannot_be_imaged() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    let reason = "multiple disks found: [\"sda\", \"sdb\"]".to_string();

    let mut events = connect(&agent, &Err(reason.clone()), watch::channel(()).1).await;
    assert_eq!(next(&mut events).await, ServerEvent::Current(None));

    let host = registered_host(s, &mac).await;
    assert_eq!(host.disk, Err(reason.clone()));
    assert!(
        s.container
            .host_registry
            .connected_hosts()
            .contains(&host.id),
        "the machine is listed as connected"
    );
    match host.usable_disk() {
        Err(AppError::FailedPrecondition(message)) => {
            assert!(message.ends_with(&reason), "{message}")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }

    drop(events);
    let _reconnected = connect(&agent, &usable(), watch::channel(()).1).await;
    assert_eq!(registered_host(s, &mac).await.disk, Ok(DISK));
}

#[tokio::test]
async fn a_stream_must_report_either_a_disk_size_or_why_there_is_none() {
    let s = harness::server().await;
    let ws_base = s.base_url.replacen("http://", "ws://", 1);
    for query in ["", "?disk_size_bytes=1&disk_error=x"] {
        let mac = harness::unique_mac();
        let mut request = format!("{ws_base}{}{query}", route::STREAM)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert(MAC_HEADER, mac.parse().unwrap());
        assert!(
            tokio_tungstenite::connect_async(request).await.is_err(),
            "the stream accepted {query:?}"
        );
        let unknown = s
            .on_server(async move { s.container.host_repo.get_by_mac(&mac).await })
            .await;
        assert!(
            matches!(unknown, Err(AppError::NotFound(_))),
            "{query:?} registered a host: {unknown:?}"
        );
    }
}
