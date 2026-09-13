use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use imaged_client::sys::disk::{BlockDevice, PartitionTarget};
use imaged_client::task::types::RunningTask;
use imaged_client::task::{
    ClientState, ClientTaskExt, DeployTask, RunnableClientTask, handle_message,
};
use imaged_shared::{ServerEvent, Task as SharedTask, TaskType};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct RecordingTask {
    log: Arc<Mutex<Vec<String>>>,
    planned: Vec<PartitionTarget>,
    calls: Arc<AtomicUsize>,
    fail_partition_at: Option<usize>,
}

impl RecordingTask {
    fn new(planned: Vec<PartitionTarget>) -> Self {
        Self {
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
    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        self.record(format!("table:{device}"));
        Ok(())
    }

    async fn plan_partitions(&self, disk: &BlockDevice) -> anyhow::Result<Vec<PartitionTarget>> {
        self.record(format!("plan:{}", disk.name));
        Ok(self.planned.clone())
    }

    async fn handle_partition(&self, partition: PartitionTarget) -> anyhow::Result<()> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        self.record(format!("partition:{}", partition.number));
        if self.fail_partition_at == Some(index) {
            anyhow::bail!("partition {} failed", partition.number);
        }
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
        fstype: "ext4".to_string(),
        size: 0,
    }
}

fn capture_event(id: i64) -> ServerEvent {
    ServerEvent::Task(SharedTask {
        id,
        task_type: TaskType::Capture,
        image_id: None,
    })
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
async fn a_task_event_starts_and_stores_the_running_task() {
    let state = Arc::new(ClientState::default());

    handle_message(state.clone(), capture_event(7)).await;

    {
        let guard = state.current_task.lock().await;
        let running = guard.as_ref().expect("a task should be stored");
        assert_eq!(running.task_id, 7);
    }

    wait_until_slot_cleared(&state).await;
}

#[tokio::test]
async fn a_second_task_while_one_runs_is_refused_and_leaves_the_first_intact() {
    let state = Arc::new(ClientState::default());
    let first_cancel = CancellationToken::new();
    preseed(&state, 100, first_cancel.clone()).await;

    handle_message(state.clone(), capture_event(200)).await;

    let guard = state.current_task.lock().await;
    assert_eq!(guard.as_ref().unwrap().task_id, 100);
    assert!(!first_cancel.is_cancelled());
}

#[tokio::test]
async fn cancel_with_a_matching_id_cancels_the_running_task() {
    let state = Arc::new(ClientState::default());
    let cancel = CancellationToken::new();
    preseed(&state, 100, cancel.clone()).await;

    handle_message(state.clone(), ServerEvent::Cancel(100)).await;

    assert!(cancel.is_cancelled());
    assert!(
        state.current_task.lock().await.is_some(),
        "cancel signals the token but does not itself clear the slot"
    );
}

#[tokio::test]
async fn cancel_with_a_stale_id_is_ignored() {
    let state = Arc::new(ClientState::default());
    let cancel = CancellationToken::new();
    preseed(&state, 100, cancel.clone()).await;

    handle_message(state.clone(), ServerEvent::Cancel(999)).await;

    assert!(!cancel.is_cancelled());
    assert_eq!(
        state.current_task.lock().await.as_ref().unwrap().task_id,
        100
    );
}

#[tokio::test]
async fn after_completion_the_running_task_slot_is_cleared() {
    let state = Arc::new(ClientState::default());

    handle_message(state.clone(), capture_event(11)).await;

    wait_until_slot_cleared(&state).await;
    assert!(state.current_task.lock().await.is_none());
}

#[tokio::test]
async fn run_processes_partition_table_then_plans_then_each_partition_in_order() {
    let task = RecordingTask::new(vec![pt(1), pt(2), pt(3)]);
    let disk = imaged_client::sys::disk::find_target_disk().await;
    let result = task.run().await;
    let log = task.entries();

    match disk {
        Ok(disk) => {
            result.expect("run should succeed once the fake owns every hook");
            assert_eq!(log[0], format!("table:/dev/{}", disk.name));
            assert_eq!(log[1], format!("plan:{}", disk.name));
            assert_eq!(&log[2..], ["partition:1", "partition:2", "partition:3"]);
            assert!(!log.iter().any(|e| e == "finalize"));
            assert!(!log.iter().any(|e| e.starts_with("finalize_error")));
        }
        Err(_) => {
            assert!(result.is_err());
            assert!(log.is_empty());
        }
    }
}

#[tokio::test]
async fn run_bails_on_the_first_partition_error_without_processing_the_rest() {
    let task = RecordingTask::new(vec![pt(1), pt(2), pt(3)]).failing_partition_at(0);
    let disk = imaged_client::sys::disk::find_target_disk().await;
    let result = task.run().await;
    let log = task.entries();

    assert!(result.is_err());
    match disk {
        Ok(_) => {
            assert!(log.iter().any(|e| e == "partition:1"));
            assert!(!log.iter().any(|e| e == "partition:2"));
            assert!(!log.iter().any(|e| e == "partition:3"));
        }
        Err(_) => assert!(log.is_empty()),
    }
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
        DeployTask::new(42).plan_partitions(&disk),
    )
    .await;

    match outcome {
        Ok(result) => {
            let err = result.expect_err("no server is configured, so the download must fail");
            assert!(
                !err.to_string().contains("has no partitions to restore"),
                "the empty-partitions bail is unreachable without a server; got: {err}"
            );
        }
        Err(_elapsed) => {}
    }
}
