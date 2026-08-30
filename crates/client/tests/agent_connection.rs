mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{StreamBehavior, StubConfig, StubServer, fake_bins, init_transport, stub_server};
use imaged_client::connection::{self, Backoff};
use imaged_client::task::ClientState;
use imaged_shared::{ServerEvent, Task, TaskType};
use tokio_util::sync::CancellationToken;

const MAC: &str = "aa:bb:cc:dd:ee:ff";
const TASK_ID: i64 = 7;

// Tight backoff so reconnects happen in milliseconds instead of the production 1s..30s.
const TEST_BACKOFF: Backoff = Backoff {
    start: Duration::from_millis(5),
    cap: Duration::from_millis(20),
};

// A fake `lsblk` that blocks before reporting no usable disks. The block keeps
// `find_target_disk` (the first thing every task's `run` awaits) pending, so a started
// task stays registered in `ClientState` for the duration of the test; the empty result
// means that if it ever unblocks it bails safely without touching any hardware.
fn install_blocking_lsblk(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("lsblk");
    std::fs::write(&path, "#!/bin/sh\nsleep 30\necho '{\"blockdevices\":[]}'\n")
        .expect("write fake lsblk");
    let mut perms = std::fs::metadata(&path).expect("stat fake lsblk").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod fake lsblk");
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
    let fakes = fake_bins();
    install_blocking_lsblk(fakes.dir());

    let task = Task::new(TASK_ID, TaskType::Deploy, Some(1));
    let server = stub_server(
        StubConfig::new()
            .with_event(ServerEvent::Task(task))
            .stream_behavior(StreamBehavior::HoldOpen),
    )
    .await;
    init_transport(&server.base_url, MAC);

    let state = Arc::new(ClientState::default());

    let driver = async {
        // The pending Task sent on connect reaches handle_message and lands in ClientState.
        wait_for_task(&state, Some(TASK_ID)).await;
        assert_eq!(server.upgrades(), 1, "the agent connected exactly once so far");
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

        // A Cancel pushed on the reconnected socket reaches handle_message and cancels the
        // running task, proving events are received on the fresh connection.
        server.send_event(ServerEvent::Cancel(TASK_ID));
        tokio::time::timeout(Duration::from_secs(5), token.cancelled())
            .await
            .expect("a Cancel on the reconnected socket must cancel the running task");
    };

    tokio::select! {
        _ = connection::run(state.clone(), 1024, TEST_BACKOFF) => {
            unreachable!("the agent must never exit because the connection dropped")
        }
        _ = driver => {}
    }
}
