mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{StreamBehavior, StubConfig, StubServer, agent_server, fake_bins, stub_server};
use imaged_agent::connection::{self, Timing};
use imaged_agent::sys::disk::HostDisk;
use imaged_agent::task::ClientState;
use imaged_model::agent::ImagePartition;
use imaged_model::host::HostPartition;
use imaged_model::progress::{HostProgress, Stage};
use imaged_model::{AgentEvent, ServerEvent, Task, Work};
use tokio_util::sync::CancellationToken;

const MAC: &str = "aa:bb:cc:dd:ee:ff";
const TASK_ID: i64 = 7;

// Tight backoff so reconnects happen in milliseconds instead of the production 250ms..5s.
// The stub never pings on its own, so the silence limit stays out of the way.
const TEST_TIMING: Timing = Timing {
    retry_start: Duration::from_millis(5),
    retry_cap: Duration::from_millis(20),
    server_silence: Duration::from_secs(60),
};

// A fake `lsblk` that blocks before reporting no usable disks. A deploy reports that it
// started and fetches the image layout, then blocks in `find_target_disk`, so a started
// task stays registered in `ClientState` for the duration of the test; the empty result
// means that if it ever unblocks it bails safely without touching any hardware.
fn install_blocking_lsblk(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("lsblk");
    std::fs::write(&path, "#!/bin/sh\nsleep 30\necho '{\"blockdevices\":[]}'\n")
        .expect("write fake lsblk");
    let mut perms = std::fs::metadata(&path)
        .expect("stat fake lsblk")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod fake lsblk");
}

fn disk() -> HostDisk {
    HostDisk {
        size_bytes: 1024,
        partitions: vec![HostPartition {
            number: 1,
            partuuid: "aaaa-01".to_string(),
            partlabel: None,
            label: Some("root".to_string()),
            uuid: Some("0b8c5c3e".to_string()),
            fstype: Some("ext4".to_string()),
            size_bytes: 512,
        }],
    }
}

fn deploy(task_id: i64) -> Task {
    Task::new(task_id, Work::Deploy { image_id: 1 })
}

async fn current_task_id(state: &ClientState) -> Option<i64> {
    state.current_task.lock().await.as_ref().map(|t| t.task_id)
}

async fn running_token(state: &ClientState) -> Option<CancellationToken> {
    state
        .current_task
        .lock()
        .await
        .as_ref()
        .map(|t| t.cancel.clone())
}

async fn wait_for_task(state: &ClientState, expected: Option<i64>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if current_task_id(state).await == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for current task {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn wait_for_agent_events(server: &StubServer, at_least: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while server.agent_events().len() < at_least {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {at_least} agent events (saw {:?})",
            server.agent_events()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn wait_for_upgrades(server: &StubServer, at_least: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if server.upgrades() >= at_least {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {at_least} upgrades (saw {})",
            server.upgrades()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn agent_dispatches_events_and_reconnects_without_exiting_or_restarting_the_running_task() {
    let fakes = fake_bins().await;
    install_blocking_lsblk(fakes.dir());

    let server = stub_server(
        StubConfig::new()
            .with_partitions(vec![ImagePartition {
                id: 1,
                partition_number: 1,
                fstype: "ext4".to_string(),
                size_bytes: 512,
                fs_uuid: None,
                fs_label: None,
            }])
            .with_event(ServerEvent::Current(Some(deploy(TASK_ID))))
            .stream_behavior(StreamBehavior::HoldOpen),
    )
    .await;
    let agent = agent_server(&server.base_url, MAC);

    let state = Arc::new(ClientState::default());

    let driver = async {
        // The current task sent on connect reaches handle_message and lands in ClientState.
        wait_for_task(&state, Some(TASK_ID)).await;
        assert_eq!(
            server.upgrades(),
            1,
            "the agent connected exactly once so far"
        );
        let token = running_token(&state)
            .await
            .expect("the delivered task must be registered as the running task");
        assert!(!token.is_cancelled());

        // The server drops the socket. The agent must reconnect (a second upgrade) rather
        // than exit, and must NOT restart the task that is still running locally, even
        // though the server re-sends it as the first message on the new connection.
        server.close_connections();
        wait_for_upgrades(&server, 2).await;
        assert_eq!(
            current_task_id(&state).await,
            Some(TASK_ID),
            "reconnect must not drop or restart the already-running task"
        );
        assert!(
            !token.is_cancelled(),
            "the running task must be untouched by the reconnect"
        );

        // The server naming no current task (the task was cancelled) on the reconnected
        // socket cancels the running task, proving events are received on the fresh connection.
        server.send_event(ServerEvent::Current(None));
        tokio::time::timeout(Duration::from_secs(5), token.cancelled())
            .await
            .expect("no current task on the reconnected socket must cancel the running task");

        // Once the slot is free the agent asks for its next task, and starts whatever the
        // server then names: a task queued while the agent was busy is not lost.
        wait_for_task(&state, None).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !server.agent_events().contains(&AgentEvent::Idle) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "an idle agent must ask for its next task, sent {:?}",
                server.agent_events()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        server.send_event(ServerEvent::Current(Some(deploy(TASK_ID + 1))));
        wait_for_task(&state, Some(TASK_ID + 1)).await;
    };

    tokio::select! {
        _ = connection::run(agent, state.clone(), async || Ok(disk()), TEST_TIMING) => {
            unreachable!("the agent must never exit because the connection dropped")
        }
        _ = driver => {}
    }
}

#[tokio::test]
async fn the_partitions_and_the_latest_progress_are_sent_on_every_connect() {
    let server = stub_server(StubConfig::new().stream_behavior(StreamBehavior::HoldOpen)).await;
    let agent = agent_server(&server.base_url, MAC);
    agent.status.task_started(&deploy(TASK_ID));
    let partitions = AgentEvent::Partitions(disk().partitions);
    let reported = AgentEvent::Progress(HostProgress {
        task_id: TASK_ID,
        fraction: 0.0,
        stage: Stage::PartitionTable,
    });

    let driver = async {
        wait_for_agent_events(&server, 2).await;
        server.close_connections();
        wait_for_upgrades(&server, 2).await;
        wait_for_agent_events(&server, 4).await;
        assert_eq!(
            server.agent_events(),
            [partitions.clone(), reported.clone(), partitions, reported]
        );
    };

    tokio::select! {
        _ = connection::run(agent.clone(), Arc::new(ClientState::default()), async || Ok(disk()), TEST_TIMING) => {
            unreachable!("the agent must never exit because the connection dropped")
        }
        _ = driver => {}
    }
}

#[tokio::test]
async fn an_agent_without_a_usable_disk_reports_no_partitions() {
    let server = stub_server(StubConfig::new().stream_behavior(StreamBehavior::HoldOpen)).await;
    let agent = agent_server(&server.base_url, MAC);
    agent.status.task_started(&deploy(TASK_ID));
    let unusable = async || Err("multiple disks found".to_string());

    let driver = async {
        wait_for_agent_events(&server, 1).await;
        assert!(
            matches!(server.agent_events().as_slice(), [AgentEvent::Progress(_)]),
            "{:?}",
            server.agent_events()
        );
    };

    tokio::select! {
        _ = connection::run(agent.clone(), Arc::new(ClientState::default()), unusable, TEST_TIMING) => {
            unreachable!("the agent must never exit because the connection dropped")
        }
        _ = driver => {}
    }
}

#[tokio::test]
async fn a_server_that_goes_silent_is_given_up_on_and_reconnected_to() {
    let server = stub_server(StubConfig::new().stream_behavior(StreamBehavior::Silent)).await;
    let agent = agent_server(&server.base_url, MAC);
    let timing = Timing {
        server_silence: Duration::from_millis(200),
        ..TEST_TIMING
    };

    tokio::select! {
        _ = connection::run(agent, Arc::new(ClientState::default()), async || Ok(disk()), timing) => {
            unreachable!("the agent must never exit because the server went silent")
        }
        _ = wait_for_upgrades(&server, 3) => {}
    }
}
