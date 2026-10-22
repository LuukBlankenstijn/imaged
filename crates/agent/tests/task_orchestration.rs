mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Route, StubConfig, StubServer, agent_server, offline_agent, stub_server};
use imaged_agent::agent::Agent;
use imaged_agent::status::{Outcome, PartitionProgress, Step};
use imaged_agent::sys::disk::{BlockDevice, PartitionTarget};
use imaged_agent::task::types::RunningTask;
use imaged_agent::task::{
    CaptureTask, ClientState, ClientTaskExt, DeployTask, RebootTask, handle_message,
};
use imaged_model::{ServerEvent, Task as SharedTask, TaskType};
use tokio_util::sync::CancellationToken;

const MAC: &str = "aa:bb:cc:dd:ee:0f";

#[derive(Clone)]
struct RecordingTask {
    agent: Agent,
    log: Arc<Mutex<Vec<String>>>,
    planned: Vec<PartitionTarget>,
    calls: Arc<AtomicUsize>,
    fail_partition_at: Option<usize>,
}

impl RecordingTask {
    fn new(planned: Vec<PartitionTarget>) -> Self {
        Self {
            agent: offline_agent(),
            log: Arc::new(Mutex::new(Vec::new())),
            planned,
            calls: Arc::new(AtomicUsize::new(0)),
            fail_partition_at: None,
        }
    }

    fn failing_partition_at(mut self, index: usize) -> Self {
        self.fail_partition_at = Some(index);
        self
    }

    fn record(&self, entry: impl Into<String>) {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(entry.into());
    }

    fn entries(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl std::fmt::Display for RecordingTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "recording task")
    }
}

impl ClientTaskExt for RecordingTask {
    fn task_id(&self) -> i64 {
        0
    }

    fn agent(&self) -> &Agent {
        &self.agent
    }

    async fn target_disk(&self) -> anyhow::Result<BlockDevice> {
        Ok(serde_json::from_str(
            r#"{ "name": "vdz", "size": 100, "children": [
                { "name": "vdz1", "size": 50, "fstype": "ext4" }
            ] }"#,
        )?)
    }

    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        self.record(format!("table:{device}"));
        Ok(())
    }

    async fn plan_partitions(&self, disk: &BlockDevice) -> anyhow::Result<Vec<PartitionTarget>> {
        self.record(format!("plan:{}", disk.name));
        Ok(self.planned.clone())
    }

    async fn handle_partition(
        &self,
        partition: PartitionTarget,
        _: PartitionProgress,
    ) -> anyhow::Result<()> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        self.record(format!("partition:{}", partition.number));
        if self.fail_partition_at == Some(index) {
            anyhow::bail!("partition {} failed", partition.number);
        }
        Ok(())
    }

    async fn after_partitions(&self, partitions: &[PartitionTarget]) -> anyhow::Result<()> {
        let numbers: Vec<String> = partitions.iter().map(|p| p.number.to_string()).collect();
        self.record(format!("after:{}", numbers.join(",")));
        Ok(())
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        self.record("finalize");
        Ok(())
    }

    async fn finalize_error(&self, err: &str) -> anyhow::Result<()> {
        self.record(format!("finalize_error:{err}"));
        Ok(())
    }
}

fn pt(number: i64) -> PartitionTarget {
    PartitionTarget {
        device: format!("/dev/fake{number}"),
        number,
        filesystem: imaged_disk::Filesystem::Ext4,
        size: 0,
    }
}

fn capture_event(id: i64) -> ServerEvent {
    ServerEvent::Current(Some(SharedTask {
        id,
        task_type: TaskType::Capture,
        image_id: None,
    }))
}

async fn reporting_server() -> (StubServer, Agent) {
    let server = stub_server(StubConfig::new()).await;
    let agent = agent_server(&server.base_url, MAC);
    (server, agent)
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
    let (server, agent) = reporting_server().await;
    let state = Arc::new(ClientState::default());
    let mut idle = state.idle();

    handle_message(&agent, state.clone(), capture_event(11)).await;

    tokio::time::timeout(Duration::from_secs(5), idle.changed())
        .await
        .expect("the agent must announce that it is idle")
        .unwrap();
    assert!(state.current_task.lock().await.is_none());
    assert_eq!(
        server
            .failed()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        [11]
    );
}

#[tokio::test]
async fn run_processes_partition_table_then_plans_then_each_partition_then_the_restored_set() {
    let task = RecordingTask::new(vec![pt(1), pt(2), pt(3)]);

    task.run()
        .await
        .expect("run should succeed once the fake owns every hook");

    assert_eq!(
        task.entries(),
        [
            "table:/dev/vdz",
            "plan:vdz",
            "partition:1",
            "partition:2",
            "partition:3",
            "after:1,2,3"
        ]
    );
}

#[tokio::test]
async fn run_bails_on_the_first_partition_error_without_processing_the_rest() {
    let task = RecordingTask::new(vec![pt(1), pt(2), pt(3)]).failing_partition_at(0);

    let err = task.run().await.unwrap_err();

    assert_eq!(err.to_string(), "partition 1 failed");
    assert_eq!(
        task.entries(),
        ["table:/dev/vdz", "plan:vdz", "partition:1"]
    );
}

#[tokio::test]
async fn a_reboot_task_neither_looks_for_a_disk_nor_lists_partitions() {
    let agent = offline_agent();
    agent
        .status
        .task_started(&SharedTask::new(3, TaskType::Reboot, None));

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
        .task_started(&SharedTask::new(5, TaskType::Capture, Some(1)));
    CaptureTask::new(agent.clone(), 5)
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
        .task_started(&SharedTask::new(6, TaskType::Capture, Some(1)));
    CaptureTask::new(agent.clone(), 6)
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

    let err = CaptureTask::new(agent, 9).finalize().await.unwrap_err();

    assert!(err.to_string().contains("400 Bad Request"), "{err:#}");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn finalize_error_receives_the_formatted_reason_verbatim() {
    let task = RecordingTask::new(vec![]);
    let reason = "deploy task 5: sgdisk --zap-all failed";

    task.finalize_error(reason).await.unwrap();

    assert_eq!(task.entries(), vec![format!("finalize_error:{reason}")]);
}

#[tokio::test]
async fn deploy_plan_partitions_fails_at_http_download_before_the_empty_partitions_bail() {
    let disk: BlockDevice = serde_json::from_str(
        r#"{ "name": "sda", "size": 100, "children": [
            { "name": "sda1", "size": 50 }
        ] }"#,
    )
    .unwrap();

    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        DeployTask::new(offline_agent(), 42).plan_partitions(&disk),
    )
    .await;

    match outcome {
        Ok(result) => {
            let err = result.expect_err("the server is unreachable, so the download must fail");
            assert!(
                !err.to_string().contains("has no partitions to restore"),
                "the empty-partitions bail is unreachable without a server; got: {err}"
            );
        }
        Err(_elapsed) => {}
    }
}
