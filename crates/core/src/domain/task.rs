use crate::error::{AppError, Result};
use chrono::{DateTime, Utc};
use derive_more::{Display, FromStr, IsVariant};
use serde::Deserialize;
use std::str::FromStr as _;

#[derive(Debug, Clone, Copy, Display, FromStr, IsVariant, PartialEq)]
#[display(rename_all = "lowercase")]
pub enum TaskType {
    Capture,
    Deploy,
    Multicast,
    Reboot,
}

impl From<TaskType> for imaged_shared::TaskType {
    fn from(value: TaskType) -> Self {
        match value {
            TaskType::Capture => imaged_shared::TaskType::Capture,
            TaskType::Deploy => imaged_shared::TaskType::Deploy,
            TaskType::Multicast => imaged_shared::TaskType::Multicast,
            TaskType::Reboot => imaged_shared::TaskType::Reboot,
        }
    }
}

impl TaskType {
    pub fn from_string(value: String) -> Result<Self> {
        TaskType::from_str(&value).map_err(|e| {
            tracing::info!(err=%e, "failed to convert string to TaskType");
            AppError::Internal("conversion error".to_string())
        })
    }
}

#[derive(Debug, Display, FromStr, IsVariant, PartialEq, Eq, Clone, Copy, Deserialize)]
#[display(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
    // Rollup-only: produced by `Task::aggregate_state`, never stored on a
    // per-host row.
    Partial,
}

impl TaskState {
    pub fn from_string(value: String) -> Result<Self> {
        TaskState::from_str(&value).map_err(|e| {
            tracing::info!(err=%e, "failed to convert string to TaskState");
            AppError::Internal("conversion error".to_string())
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskHost {
    pub host_id: i64,
    pub state: TaskState,
    pub error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: i64,
    pub task_type: TaskType,
    pub hosts: Vec<TaskHost>,
    pub image_id: Option<i64>,
    pub image_name: Option<String>,
    pub image_deleted: bool,
    pub created_at: DateTime<Utc>,
}

impl Task {
    /// Derives the task-level status from the per-host states. `Partial` means
    /// some hosts finished while others failed/were cancelled. For a
    /// single-host task this equals that host's state.
    pub fn aggregate_state(&self) -> TaskState {
        use TaskState::*;
        if self.hosts.is_empty() {
            // Every host was deleted (rows CASCADE away). The task is defunct —
            // report it terminal so a finished task isn't resurrected as active.
            return Cancelled;
        }
        let states = || self.hosts.iter().map(|h| h.state);

        if states().any(|s| matches!(s, Pending | Running)) {
            // Something is still in flight: Running if any host has started,
            // otherwise nothing has begun yet.
            return if states().any(|s| s != Pending) {
                Running
            } else {
                Pending
            };
        }

        // Every host is terminal.
        if states().all(|s| s == Done) {
            return Done;
        }
        if states().all(|s| s == Cancelled) {
            return Cancelled;
        }
        if !states().any(|s| s == Done) {
            // No successes, only failures/cancellations.
            return Failed;
        }
        Partial
    }
}

#[async_trait::async_trait]
pub trait TaskRepository: Send + Sync {
    // create a new task
    async fn create(
        &self,
        task_type: TaskType,
        host_ids: Vec<i64>,
        image_id: Option<i64>,
    ) -> Result<Task>;

    // gets the next task for a host. This takes Pending and Running host rows into account
    async fn get_next(&self, host_id: i64) -> Result<Option<Task>>;

    // marks one host's row of a task as running
    async fn start(&self, task_id: i64, host_id: i64) -> Result;

    // marks one host's row of a task as done
    async fn mark_finished(&self, task_id: i64, host_id: i64) -> Result;

    // marks one host's row of a task as failed
    async fn mark_failed(&self, task_id: i64, host_id: i64, error: &str) -> Result;

    // marks every host row of a task as done (multicast fan-out)
    async fn mark_all_finished(&self, task_id: i64) -> Result;

    // marks every host row of a task as failed (multicast fan-out)
    async fn mark_all_failed(&self, task_id: i64, error: &str) -> Result;

    // reset a task's failed/cancelled host rows to pending
    async fn retry(&self, id: i64) -> Result;

    // cancel a task's running or pending host rows
    async fn cancel(&self, id: i64) -> Result;

    // gets all active tasks for some image
    async fn get_active_by_image(&self, image_id: i64) -> Result<Vec<Task>>;

    // gets all tasks
    async fn get_all(&self) -> Result<Vec<Task>>;

    async fn get(&self, id: i64) -> Result<Task>;

    async fn get_next_multicast(&self) -> Result<Option<Task>>;
}
