use derive_more::{Constructor, Display, From, IsVariant};
use serde::{Deserialize, Serialize};

use crate::host::HostPartition;
use crate::progress::HostProgress;

#[derive(Debug, Display, IsVariant, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum TaskType {
    Capture,
    CapturePartition,
    Deploy,
    DeployPartition,
    Multicast,
    MulticastPartition,
    Reboot,
}

#[derive(Debug, Serialize, Deserialize, Constructor, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: i64,
    pub work: Work,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum Work {
    Capture { image_id: i64 },
    CapturePartition { image_id: i64, partuuid: String },
    Deploy { image_id: i64 },
    Multicast { image_id: i64 },
    DeployPartition { image_id: i64, partuuid: String },
    MulticastPartition { image_id: i64, partuuid: String },
    Reboot,
}

impl Work {
    pub fn task_type(&self) -> TaskType {
        match self {
            Self::Capture { .. } => TaskType::Capture,
            Self::CapturePartition { .. } => TaskType::CapturePartition,
            Self::Deploy { .. } => TaskType::Deploy,
            Self::Multicast { .. } => TaskType::Multicast,
            Self::DeployPartition { .. } => TaskType::DeployPartition,
            Self::MulticastPartition { .. } => TaskType::MulticastPartition,
            Self::Reboot => TaskType::Reboot,
        }
    }

    pub fn image_id(&self) -> Option<i64> {
        match self {
            Self::Capture { image_id }
            | Self::CapturePartition { image_id, .. }
            | Self::Deploy { image_id }
            | Self::Multicast { image_id }
            | Self::DeployPartition { image_id, .. }
            | Self::MulticastPartition { image_id, .. } => Some(*image_id),
            Self::Reboot => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, From, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Current(Option<Task>),
}

#[derive(Debug, Serialize, Deserialize, From, Clone, PartialEq)]
pub enum AgentEvent {
    Progress(HostProgress),
    Partitions(Vec<HostPartition>),
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
    fn server_event_current_round_trips_with_and_without_a_task() {
        let partition = Task::new(
            5,
            Work::DeployPartition {
                image_id: 3,
                partuuid: "PARTUUID".into(),
            },
        );
        for current in [Some(partition), Some(Task::new(6, Work::Reboot)), None] {
            let json = serde_json::to_string(&ServerEvent::from(current.clone())).unwrap();
            let back: ServerEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ServerEvent::Current(current));
        }
    }
}
