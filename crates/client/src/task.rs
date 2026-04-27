mod capture;
use anyhow::Result;
use tokio::{sync::Mutex, task::JoinHandle};

use std::sync::Arc;

use imaged_shared::{ServerEvent, Task, TaskType};
use tokio_util::sync::CancellationToken;

use crate::transport::ApiClient;

pub struct ClientState {
    http: crate::transport::ApiClient,
    current_task: Mutex<Option<RunningTask>>,
}

impl ClientState {
    pub fn new(base_url: String, mac: String) -> Result<Self> {
        Ok(Self {
            http: ApiClient::new(base_url, mac)?,
            current_task: Mutex::new(None),
        })
    }
}

struct RunningTask {
    task_id: i64,
    // can be used for forcefully aborting the task
    _handle: JoinHandle<()>,
    cancel: tokio_util::sync::CancellationToken,
}

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
        if let Err(e) = run_task(state_for_task.clone(), task, cancel_for_task).await {
            tracing::error!(err=%e, task_id=%task.id, "task failed");
        }
        let mut current = state_for_task.current_task.lock().await;
        *current = None;
    });

    *current = Some(RunningTask {
        task_id: task.id,
        _handle: handle,
        cancel,
    });
}

async fn run_task(
    state: Arc<ClientState>,
    task: Task,
    cancel_for_task: CancellationToken,
) -> Result<()> {
    match task.task_type {
        TaskType::Capture => {
            let result = capture::run(state.clone(), task.image_id, cancel_for_task).await;
            if let Err(ref e) = result {
                let _ = state
                    .http
                    .mark_capture_failed(task.image_id, format!("{}", e))
                    .await;
            };
            result
        }
        TaskType::Deploy => todo!(),
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
