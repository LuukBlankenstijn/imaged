pub mod implementations;
mod post_install;
pub mod types;

use std::sync::Arc;

use crate::agent::Agent;
use crate::status::Outcome;
use types::RunningTask;
pub use types::{ClientState, RunnableClientTask};

pub use implementations::{
    CaptureTask, ClientTaskExt, DeployTask, MulticastTask, RebootTask, Task,
};

use tokio_util::sync::CancellationToken;

const PARTTABLE_TMP: &str = "/parttable.bin";

pub async fn handle_message(
    agent: &Agent,
    state: Arc<ClientState>,
    msg: imaged_model::ServerEvent,
) {
    match msg {
        imaged_model::ServerEvent::Task(task) => start_task(agent, state, task).await,
        imaged_model::ServerEvent::Cancel(task_id) => state.cancel(task_id).await,
    }
}

async fn start_task(agent: &Agent, state: Arc<ClientState>, task: imaged_model::Task) {
    let mut current = state.current_task.lock().await;
    if current.is_some() {
        tracing::warn!("task already running, ignoring new task");
        return;
    }

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let state_for_task = state.clone();
    let runnable = Task::new(agent.clone(), &task);
    agent.status.task_started(&task);

    let handle = tokio::spawn(async move {
        tracing::info!(task_id=%task.id, task_type=%task.task_type, "starting task");
        run_task(runnable, cancel_for_task).await;
        let mut current = state_for_task.current_task.lock().await;
        *current = None;
    });

    *current = Some(RunningTask {
        task_id: task.id,
        _handle: handle,
        cancel,
    });
}

async fn run_task(task: Task, cancel_for_task: CancellationToken) {
    let outcome = tokio::select! {
        r = task.run() => Some(r),
        _ = cancel_for_task.cancelled() => None,
    };
    let status = task.agent().status.clone();
    let finalized = match outcome {
        None => {
            tracing::error!("task cancelled");
            status.task_ended(Outcome::Cancelled);
            return;
        }
        Some(Ok(())) => task.finalize().await,
        Some(Err(e)) => task.finalize_error(&format!("{e:#}")).await,
    };
    if let Err(e) = finalized {
        tracing::error!(task=%task, error=%e, "failed to finalize task");
        status.task_ended(Outcome::Failed(format!(
            "could not report the result: {e:#}"
        )));
    }
}
