use derive_more::{Display, FromStr, IsVariant};
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Display, FromStr, IsVariant, PartialEq, Eq, Serialize, Deserialize,
)]
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

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [TaskState; 6] = [
        TaskState::Pending,
        TaskState::Running,
        TaskState::Done,
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::Partial,
    ];

    #[test]
    fn task_state_round_trips_through_display_and_from_str_for_all_variants() {
        for s in ALL {
            assert_eq!(s.to_string().parse::<TaskState>().unwrap(), s);
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
    fn task_state_serde_round_trips_as_its_display_name() {
        for s in ALL {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{s}\""));
            assert_eq!(serde_json::from_str::<TaskState>(&json).unwrap(), s);
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
}
