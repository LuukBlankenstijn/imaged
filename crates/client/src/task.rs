mod implementations;
mod types;

use std::sync::Arc;
pub use types::ClientState;
use types::RunningTask;

use tokio_util::sync::CancellationToken;

use crate::task::{
    implementations::{ClientTaskExt, Task},
    types::RunnableClientTask,
};

const PARTTABLE_TMP: &str = "/parttable.bin";

pub async fn handle_message(state: Arc<ClientState>, msg: imaged_shared::ServerEvent) {
    match msg {
        imaged_shared::ServerEvent::Task(task) => start_task(state, task).await,
        imaged_shared::ServerEvent::Cancel(task_id) => cancel_task(state, task_id).await,
    }
}

async fn start_task(state: Arc<ClientState>, task: imaged_shared::Task) {
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
        run_task(state_for_task.clone(), task.into(), cancel_for_task).await;
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
    let result = task.run(state.clone(), cancel_for_task).await;
    match result {
        Ok(_) => {
            if let Err(e) = task.finalize(&state.http).await {
                tracing::error!(task=%task, error=%e, "failed to finalize task");
            }
        }
        Err(e) => {
            let reason = format!("{e:#}");
            if let Err(e) = task.finalize_error(&state.http, &reason).await {
                tracing::error!(task=%task, error=%e, "failed to finalize task");
            }
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
