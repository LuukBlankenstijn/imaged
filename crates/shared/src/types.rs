use derive_more::{Constructor, From, IsVariant};
use serde::{Deserialize, Serialize};

#[derive(Debug, IsVariant, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub enum TaskType {
    Capture,
    Deploy,
}

#[derive(Debug, Serialize, Deserialize, Constructor, Clone, Copy)]
pub struct Task {
    pub id: i64,
    pub task_type: TaskType,
    pub image_id: i64,
}

#[derive(Debug, Serialize, Deserialize, From, Clone, Copy)]
pub enum ServerEvent {
    Task(Task),
    Cancel(i64),
}
