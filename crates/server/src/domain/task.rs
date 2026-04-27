use crate::error::{AppError, Result};
use chrono::{DateTime, Utc};
use derive_more::{Display, FromStr, IsVariant};
use std::str::FromStr as _;

#[derive(Debug, Clone, Copy, Display, FromStr, IsVariant, PartialEq)]
#[display(rename_all = "lowercase")]
pub enum TaskType {
    Capture,
    Deploy,
}

impl From<TaskType> for imaged_shared::TaskType {
    fn from(value: TaskType) -> Self {
        match value {
            TaskType::Capture => imaged_shared::TaskType::Capture,
            TaskType::Deploy => imaged_shared::TaskType::Deploy,
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

#[derive(Debug, Display, FromStr, IsVariant, PartialEq)]
#[display(rename_all = "lowercase")]
pub enum TaskState {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn from_string(value: String) -> Result<Self> {
        TaskState::from_str(&value).map_err(|e| {
            tracing::info!(err=%e, "failed to convert string to TaskState");
            AppError::Internal("conversion error".to_string())
        })
    }
}

pub struct Task {
    pub id: i64,
    pub task_type: TaskType,
    pub host_id: Option<i64>,
    pub image_id: Option<i64>,
    pub state: TaskState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[async_trait::async_trait]
pub trait TaskRepository: Send + Sync {
    // create a new task
    async fn create(&self, task_type: TaskType, host_id: i64, image_id: i64) -> Result<Task>;

    // gets the next task for a host. This takes Pending and Running Tasks into account
    async fn get_next(&self, host_id: i64) -> Result<Option<Task>>;

    async fn start(&self, task_id: i64) -> Result;

    async fn mark_finished(&self, task_id: i64) -> Result;

    async fn mark_failed(&self, task_id: i64, error: &str) -> Result;

    // reset some running task to pending
    async fn retry(&self, id: i64) -> Result;

    // cancel a running or pending task
    async fn cancel(&self, id: i64) -> Result;

    // gets all active tasks for some image
    async fn get_active_by_image(&self, image_id: i64) -> Result<Vec<Task>>;

    // gets all tasks, optionally filtering by image_id
    async fn get_all(&self) -> Result<Vec<Task>>;

    async fn get(&self, id: i64) -> Result<Task>;
}
