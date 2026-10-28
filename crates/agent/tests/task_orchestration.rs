mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    FakeBins, Route, StubConfig, StubServer, agent_server, fake_bins, offline_agent, stub_server,
};
use imaged_agent::agent::Agent;
use imaged_agent::status::{Outcome, Step};
use imaged_agent::task::types::RunningTask;
use imaged_agent::task::{
    Capture, ClientState, ClientTaskExt, Http, RebootTask, Restore, WholeDisk, handle_message,
};
use imaged_model::agent::path;
use imaged_model::{ServerEvent, Task as SharedTask, Work};
use tokio_util::sync::CancellationToken;

const MAC: &str = "aa:bb:cc:dd:ee:0f";

fn capture_event(id: i64) -> ServerEvent {
    ServerEvent::Current(Some(SharedTask::new(id, Work::Capture { image_id: 1 })))
}

async fn reporting_server() -> (StubServer, Agent) {
    let server = stub_server(StubConfig::new()).await;
    let agent = agent_server(&server.base_url, MAC);
    (server, agent)
}

async fn without_disks() -> FakeBins {
    let fakes = fake_bins().await;
    fakes.lsblk(r#"{"blockdevices":[]}"#);
    fakes
}

async fn preseed(state: &Arc<ClientState>, task_id: i64, cancel: CancellationToken) {
    let running = RunningTask {
        task_id,
        _handle: tokio::spawn(async {}),
        cancel,
    };
    *state.current_task.lock().await = Some(running);
}

async fn wait_until_slot_cleared(state: &Arc<ClientState>) {
    for _ in 0..500 {
        if state.current_task.lock().await.is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("running-task slot never cleared");
}

#[tokio::test]
async fn a_current_task_while_idle_starts_and_stores_the_running_task() {
    let _fakes = without_disks().await;
    let (_server, agent) = reporting_server().await;
    let state = Arc::new(ClientState::default());

    handle_message(&agent, state.clone(), capture_event(7)).await;

    {
        let guard = state.current_task.lock().await;
        let running = guard.as_ref().expect("a task should be stored");
        assert_eq!(running.task_id, 7);
    }

    wait_until_slot_cleared(&state).await;
}

#[tokio::test]
async fn naming_the_running_task_again_leaves_it_running() {
    let state = Arc::new(ClientState::default());
    let cancel = CancellationToken::new();
    preseed(&state, 100, cancel.clone()).await;

    handle_message(&offline_agent(), state.clone(), capture_event(100)).await;

    assert!(!cancel.is_cancelled());
    assert_eq!(
        state.current_task.lock().await.as_ref().unwrap().task_id,
        100
    );
}

#[tokio::test]
async fn naming_another_task_cancels_the_running_one_without_starting_the_other_yet() {
    let state = Arc::new(ClientState::default());
    let cancel = CancellationToken::new();
    preseed(&state, 100, cancel.clone()).await;

    handle_message(&offline_agent(), state.clone(), capture_event(200)).await;

    assert!(cancel.is_cancelled());
    assert_eq!(
        state.current_task.lock().await.as_ref().unwrap().task_id,
        100,
        "the next task starts once the cancelled one has stopped and the agent asks again"
    );
}

#[tokio::test]
async fn naming_no_task_cancels_the_running_one() {
    let state = Arc::new(ClientState::default());
    let cancel = CancellationToken::new();
    preseed(&state, 100, cancel.clone()).await;

    handle_message(&offline_agent(), state.clone(), ServerEvent::Current(None)).await;

    assert!(cancel.is_cancelled());
    assert!(
        state.current_task.lock().await.is_some(),
        "cancel signals the token but does not itself clear the slot"
    );
}

#[tokio::test]
async fn after_completion_the_slot_is_cleared_and_the_agent_becomes_idle() {
    let _fakes = without_disks().await;
    let (server, agent) = reporting_server().await;
    let state = Arc::new(ClientState::default());
    let mut idle = state.idle();

    handle_message(&agent, state.clone(), capture_event(11)).await;

    tokio::time::timeout(Duration::from_secs(5), idle.changed())
        .await
        .expect("the agent must announce that it is idle")
        .unwrap();
    assert!(state.current_task.lock().await.is_none());
    let failed = server.failed();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, 11);
    assert!(failed[0].1.contains("no suitable disk found"), "{failed:?}");
}

#[tokio::test]
async fn a_restore_reports_that_it_started_first_and_refuses_an_image_without_partitions() {
    let (server, agent) = reporting_server().await;

    let err = Restore::new(agent, 4, Http, WholeDisk)
        .run()
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "image for task 4 has no partitions to restore"
    );
    let requests: Vec<_> = server
        .requests()
        .into_iter()
        .map(|r| (r.method, r.path, r.query))
        .collect();
    assert_eq!(
        requests,
        [
            ("POST".to_string(), path::started(4), None),
            ("GET".to_string(), path::partitions(4), None),
        ]
    );
}

#[tokio::test]
async fn a_capture_reports_the_disks_logical_sector_size_when_it_starts() {
    let fakes = fake_bins().await;
    fakes.lsblk(
        r#"{"blockdevices":[{"name":"imaged-test-missing","size":1048576,"type":"disk",
            "log-sec":4096,"children":[]}]}"#,
    );
    let (server, agent) = reporting_server().await;

    let err = Capture::new(agent, 5, WholeDisk).run().await.unwrap_err();

    assert!(
        format!("{err:#}").contains("/dev/imaged-test-missing"),
        "{err:#}"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].path, path::started(5));
    assert_eq!(requests[0].query.as_deref(), Some("sector_size=4096"));
}

#[tokio::test]
async fn a_reboot_task_neither_looks_for_a_disk_nor_lists_partitions() {
    let agent = offline_agent();
    agent.status.task_started(&SharedTask::new(3, Work::Reboot));

    RebootTask::new(agent.clone(), 3)
        .run()
        .await
        .expect("a reboot has no disk work to fail");

    let task = agent.status.snapshot().task.unwrap();
    assert_eq!(task.step, Step::Starting);
    assert!(task.partitions.is_empty());
}

#[tokio::test]
async fn the_final_report_is_retried_until_the_server_takes_it() {
    let server = stub_server(
        StubConfig::new()
            .fail_transiently(Route::Failed, 1)
            .fail_transiently(Route::Finished, 1),
    )
    .await;
    let agent = agent_server(&server.base_url, MAC);

    agent
        .status
        .task_started(&SharedTask::new(5, Work::Capture { image_id: 1 }));
    Capture::new(agent.clone(), 5, WholeDisk)
        .finalize_error("sgdisk failed")
        .await
        .expect("the failure is reported once the server recovers");
    assert_eq!(server.failed(), [(5, "sgdisk failed".to_string())]);
    assert_eq!(
        agent.status.snapshot().task.unwrap().outcome.unwrap().0,
        Outcome::Failed("sgdisk failed".into())
    );

    agent
        .status
        .task_started(&SharedTask::new(6, Work::Capture { image_id: 1 }));
    Capture::new(agent.clone(), 6, WholeDisk)
        .finalize()
        .await
        .expect("the success is reported once the server recovers");
    assert_eq!(server.finished(), [6]);
    assert_eq!(
        agent.status.snapshot().task.unwrap().outcome.unwrap().0,
        Outcome::Done
    );
}

#[tokio::test]
async fn a_final_report_the_server_refuses_is_not_retried() {
    let server = stub_server(StubConfig::new().force(Route::Finished, 400)).await;
    let agent = agent_server(&server.base_url, MAC);

    let err = Capture::new(agent, 9, WholeDisk)
        .finalize()
        .await
        .unwrap_err();

    assert!(err.to_string().contains("400 Bad Request"), "{err:#}");
    assert_eq!(server.requests().len(), 1);
}
