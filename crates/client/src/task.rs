mod capture;
mod deploy;
mod multicast;
mod types;

use std::sync::Arc;
pub use types::ClientState;
use types::RunningTask;

use imaged_shared::{ServerEvent, Task, TaskType};
use tokio_util::sync::CancellationToken;

use crate::task::types::ClientTaskExt;

const PARTTABLE_TMP: &str = "/parttable.bin";

pub async fn handle_message(state: Arc<ClientState>, msg: ServerEvent) {
    match msg {
        ServerEvent::Task(task) => start_task(state, task).await,
        ServerEvent::Cancel(task_id) => cancel_task(state, task_id).await,
    }
}

async fn start_task(state: Arc<ClientState>, task: Task) {
    let mut current = state.current_task.lock().await;
    if current.is_some() {
        tracing::warn!("task already running, ignoring new task");
        return;
    }

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let state_for_task = state.clone();

    let handle = tokio::spawn(async move {
        tracing::info!(task_id=%task.id, task_type=%task.task_type, "starting task");
        run_task(state_for_task.clone(), task, cancel_for_task).await;
        let mut current = state_for_task.current_task.lock().await;
        *current = None;
    });

    *current = Some(RunningTask {
        task_id: task.id,
        _handle: handle,
        cancel,
    });
}

async fn run_task(state: Arc<ClientState>, task: Task, cancel_for_task: CancellationToken) {
    let result = match task.task_type {
        TaskType::Capture => {
            let task = capture::CaptureTask::new(task.id);
            task.run(state.clone(), cancel_for_task).await
        }
        TaskType::Deploy => {
            let task = deploy::DeployTask::new(task.id);
            task.run(state.clone(), cancel_for_task).await
        }
        TaskType::Multicast => {
            let task = multicast::MulticastTask;
            task.run(state.clone(), cancel_for_task).await
        }
    };
    match result {
        Ok(_) => {
            if !task.task_type.is_multicast() {
                let _ = state.http.mark_task_finished(task.id).await;
            }
            tracing::info!(task_id=%task.id, task_type=%task.task_type, "task completed");
        }
        Err(e) => {
            let _ = state.http.mark_task_failed(task.id, &format!("{e}")).await;
            tracing::error!(err=%e, task_id=%task.id, task_type=%task.task_type, "task failed");
        }
    }
}

async fn cancel_task(state: Arc<ClientState>, task_id: i64) {
    let current = state.current_task.lock().await;
    if let Some(running) = current.as_ref()
        && running.task_id == task_id
    {
        running.cancel.cancel();
    }
}
