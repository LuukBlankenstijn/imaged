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

#[cfg(test)]
mod tests {
    use super::*;

    fn host(state: TaskState) -> TaskHost {
        TaskHost {
            host_id: 1,
            state,
            error: None,
            started_at: None,
            finished_at: None,
        }
    }

    fn task(states: &[TaskState]) -> Task {
        Task {
            id: 1,
            task_type: TaskType::Deploy,
            hosts: states.iter().copied().map(host).collect(),
            image_id: None,
            image_name: None,
            image_deleted: false,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn an_empty_host_list_aggregates_to_cancelled() {
        assert_eq!(task(&[]).aggregate_state(), TaskState::Cancelled);
    }

    #[test]
    fn all_pending_hosts_aggregate_to_pending() {
        assert_eq!(
            task(&[TaskState::Pending, TaskState::Pending]).aggregate_state(),
            TaskState::Pending
        );
    }

    #[test]
    fn all_running_hosts_aggregate_to_running() {
        assert_eq!(
            task(&[TaskState::Running, TaskState::Running]).aggregate_state(),
            TaskState::Running
        );
    }

    #[test]
    fn a_mix_of_pending_and_running_aggregates_to_running_because_one_has_started() {
        assert_eq!(
            task(&[TaskState::Pending, TaskState::Running]).aggregate_state(),
            TaskState::Running
        );
    }

    #[test]
    fn a_pending_host_alongside_a_done_host_aggregates_to_running() {
        assert_eq!(
            task(&[TaskState::Pending, TaskState::Done]).aggregate_state(),
            TaskState::Running
        );
    }

    #[test]
    fn all_done_hosts_aggregate_to_done() {
        assert_eq!(
            task(&[TaskState::Done, TaskState::Done]).aggregate_state(),
            TaskState::Done
        );
    }

    #[test]
    fn all_cancelled_hosts_aggregate_to_cancelled() {
        assert_eq!(
            task(&[TaskState::Cancelled, TaskState::Cancelled]).aggregate_state(),
            TaskState::Cancelled
        );
    }

    #[test]
    fn all_failed_hosts_aggregate_to_failed() {
        assert_eq!(
            task(&[TaskState::Failed, TaskState::Failed]).aggregate_state(),
            TaskState::Failed
        );
    }

    #[test]
    fn a_terminal_mix_of_only_failures_and_cancellations_aggregates_to_failed() {
        assert_eq!(
            task(&[TaskState::Failed, TaskState::Cancelled]).aggregate_state(),
            TaskState::Failed
        );
    }

    #[test]
    fn a_terminal_mix_of_done_and_failed_aggregates_to_partial() {
        assert_eq!(
            task(&[TaskState::Done, TaskState::Failed]).aggregate_state(),
            TaskState::Partial
        );
    }

    #[test]
    fn a_terminal_mix_of_cancelled_and_done_aggregates_to_partial() {
        assert_eq!(
            task(&[TaskState::Cancelled, TaskState::Done]).aggregate_state(),
            TaskState::Partial
        );
    }

    #[test]
    fn a_terminal_mix_of_done_failed_and_cancelled_aggregates_to_partial() {
        assert_eq!(
            task(&[TaskState::Done, TaskState::Failed, TaskState::Cancelled]).aggregate_state(),
            TaskState::Partial
        );
    }

    #[test]
    fn a_single_host_task_aggregates_to_that_hosts_state() {
        assert_eq!(
            task(&[TaskState::Pending]).aggregate_state(),
            TaskState::Pending
        );
        assert_eq!(
            task(&[TaskState::Running]).aggregate_state(),
            TaskState::Running
        );
        assert_eq!(task(&[TaskState::Done]).aggregate_state(), TaskState::Done);
        assert_eq!(
            task(&[TaskState::Failed]).aggregate_state(),
            TaskState::Failed
        );
        assert_eq!(
            task(&[TaskState::Cancelled]).aggregate_state(),
            TaskState::Cancelled
        );
    }

    #[test]
    fn aggregate_state_never_yields_partial_from_a_single_host() {
        for s in [
            TaskState::Pending,
            TaskState::Running,
            TaskState::Done,
            TaskState::Failed,
            TaskState::Cancelled,
        ] {
            assert_ne!(task(&[s]).aggregate_state(), TaskState::Partial);
        }
    }

    #[test]
    fn task_state_is_variant_predicates_are_exhaustive_and_mutually_exclusive() {
        assert!(TaskState::Pending.is_pending());
        assert!(TaskState::Running.is_running());
        assert!(TaskState::Done.is_done());
        assert!(TaskState::Failed.is_failed());
        assert!(TaskState::Cancelled.is_cancelled());
        assert!(TaskState::Partial.is_partial());

        assert!(!TaskState::Pending.is_running());
        assert!(!TaskState::Done.is_partial());
        assert!(!TaskState::Partial.is_done());
        assert!(!TaskState::Failed.is_cancelled());
        assert!(!TaskState::Cancelled.is_failed());
    }

    #[test]
    fn task_type_round_trips_through_display_and_from_string() {
        for t in [
            TaskType::Capture,
            TaskType::Deploy,
            TaskType::Multicast,
            TaskType::Reboot,
        ] {
            assert_eq!(TaskType::from_string(t.to_string()).unwrap(), t);
        }
    }

    #[test]
    fn task_type_display_is_lowercase() {
        assert_eq!(TaskType::Capture.to_string(), "capture");
        assert_eq!(TaskType::Deploy.to_string(), "deploy");
        assert_eq!(TaskType::Multicast.to_string(), "multicast");
        assert_eq!(TaskType::Reboot.to_string(), "reboot");
    }

    #[test]
    fn task_type_from_string_is_case_insensitive_as_derive_more_implements_it() {
        assert_eq!(
            TaskType::from_string("Capture".to_string()).unwrap(),
            TaskType::Capture
        );
        assert_eq!(
            TaskType::from_string("CAPTURE".to_string()).unwrap(),
            TaskType::Capture
        );
        assert_eq!(
            TaskType::from_string("dEpLoY".to_string()).unwrap(),
            TaskType::Deploy
        );
    }

    #[test]
    fn task_type_from_string_rejects_unknown_with_internal_conversion_error() {
        let err = TaskType::from_string("bogus".to_string()).unwrap_err();
        match err {
            AppError::Internal(msg) => assert_eq!(msg, "conversion error"),
            other => panic!("expected AppError::Internal, got {other:?}"),
        }
    }

    #[test]
    fn task_state_round_trips_through_display_and_from_string_for_all_variants() {
        for s in [
            TaskState::Pending,
            TaskState::Running,
            TaskState::Done,
            TaskState::Failed,
            TaskState::Cancelled,
            TaskState::Partial,
        ] {
            assert_eq!(TaskState::from_string(s.to_string()).unwrap(), s);
        }
    }

    #[test]
    fn task_state_display_is_lowercase() {
        assert_eq!(TaskState::Pending.to_string(), "pending");
        assert_eq!(TaskState::Running.to_string(), "running");
        assert_eq!(TaskState::Done.to_string(), "done");
        assert_eq!(TaskState::Failed.to_string(), "failed");
        assert_eq!(TaskState::Cancelled.to_string(), "cancelled");
        assert_eq!(TaskState::Partial.to_string(), "partial");
    }

    #[test]
    fn task_state_from_string_rejects_unknown_with_internal_conversion_error() {
        let err = TaskState::from_string("bogus".to_string()).unwrap_err();
        match err {
            AppError::Internal(msg) => assert_eq!(msg, "conversion error"),
            other => panic!("expected AppError::Internal, got {other:?}"),
        }
    }

    #[test]
    fn task_type_converts_exhaustively_into_shared_task_type() {
        assert_eq!(
            imaged_shared::TaskType::from(TaskType::Capture),
            imaged_shared::TaskType::Capture
        );
        assert_eq!(
            imaged_shared::TaskType::from(TaskType::Deploy),
            imaged_shared::TaskType::Deploy
        );
        assert_eq!(
            imaged_shared::TaskType::from(TaskType::Multicast),
            imaged_shared::TaskType::Multicast
        );
        assert_eq!(
            imaged_shared::TaskType::from(TaskType::Reboot),
            imaged_shared::TaskType::Reboot
        );
    }
}
