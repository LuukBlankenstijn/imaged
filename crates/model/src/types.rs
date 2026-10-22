use derive_more::{Constructor, Display, From, IsVariant};
use serde::{Deserialize, Serialize};

use crate::progress::HostProgress;

#[derive(Debug, Display, IsVariant, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum TaskType {
    Capture,
    Deploy,
    Multicast,
    Reboot,
}

#[derive(Debug, Serialize, Deserialize, Constructor, Clone, Copy, PartialEq, Eq)]
pub struct Task {
    pub id: i64,
    pub task_type: TaskType,
    pub image_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, From, Clone, Copy, PartialEq, Eq)]
pub enum ServerEvent {
    Current(Option<Task>),
}

#[derive(Debug, Serialize, Deserialize, From, Clone, PartialEq)]
pub enum AgentEvent {
    Progress(HostProgress),
    #[from(skip)]
    Idle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_type_display_uses_the_variant_name() {
        assert_eq!(TaskType::Capture.to_string(), "Capture");
        assert_eq!(TaskType::Deploy.to_string(), "Deploy");
        assert_eq!(TaskType::Multicast.to_string(), "Multicast");
        assert_eq!(TaskType::Reboot.to_string(), "Reboot");
    }

    #[test]
    fn task_type_is_variant_helpers_discriminate_variants() {
        assert!(TaskType::Capture.is_capture());
        assert!(!TaskType::Capture.is_deploy());
        assert!(TaskType::Reboot.is_reboot());
        assert!(!TaskType::Reboot.is_multicast());
    }

    #[test]
    fn task_type_serde_round_trips_as_its_name() {
        let json = serde_json::to_string(&TaskType::Multicast).unwrap();
        assert_eq!(json, "\"Multicast\"");
        let back: TaskType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TaskType::Multicast);
    }

    #[test]
    fn task_constructor_sets_all_fields() {
        let t = Task::new(7, TaskType::Deploy, Some(3));
        assert_eq!(t.id, 7);
        assert_eq!(t.task_type, TaskType::Deploy);
        assert_eq!(t.image_id, Some(3));
    }

    #[test]
    fn task_serde_round_trips_all_fields() {
        let t = Task::new(9, TaskType::Capture, None);
        let json = serde_json::to_string(&t).unwrap();
        let back: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 9);
        assert_eq!(back.task_type, TaskType::Capture);
        assert_eq!(back.image_id, None);
    }

    #[test]
    fn server_event_current_round_trips_with_and_without_a_task() {
        for current in [Some(Task::new(5, TaskType::Capture, None)), None] {
            let json = serde_json::to_string(&ServerEvent::from(current)).unwrap();
            let back: ServerEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ServerEvent::Current(current));
        }
    }
}
