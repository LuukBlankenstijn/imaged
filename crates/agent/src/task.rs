pub mod implementations;
mod post_install;
pub mod types;

use std::sync::Arc;

use crate::agent::Agent;
use crate::status::Outcome;
pub use types::ClientState;
use types::RunningTask;

pub use implementations::{
    CaptureTask, ClientTaskExt, DeployTask, MulticastTask, RebootTask, Task,
};

use tokio_util::sync::CancellationToken;

const PARTTABLE_TMP: &str = "/parttable.bin";

pub async fn handle_message(
    agent: &Agent,
    state: Arc<ClientState>,
    event: imaged_model::ServerEvent,
) {
    let imaged_model::ServerEvent::Current(current) = event;
    let mut running = state.current_task.lock().await;
    match (running.as_ref(), current) {
        (Some(task), Some(current)) if task.task_id == current.id => {}
        (Some(task), _) => {
            tracing::warn!(
                task_id = task.task_id,
                "the server no longer lists the running task as current, cancelling it"
            );
            task.cancel.cancel();
        }
        (None, Some(current)) => *running = Some(start_task(agent, state.clone(), current)),
        (None, None) => {}
    }
}

fn start_task(agent: &Agent, state: Arc<ClientState>, task: imaged_model::Task) -> RunningTask {
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let runnable = Task::new(agent.clone(), &task);
    agent.status.task_started(&task);

    let handle = tokio::spawn(async move {
        tracing::info!(task_id=%task.id, task_type=%task.task_type, "starting task");
        run_task(runnable, cancel_for_task).await;
        *state.current_task.lock().await = None;
        state.became_idle();
    });

    RunningTask {
        task_id: task.id,
        _handle: handle,
        cancel,
    }
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
