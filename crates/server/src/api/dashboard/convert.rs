use std::time::SystemTime;

use imaged_rpc::dashboard::v1 as pb;
use prost_types::Timestamp;

use crate::{
    domain::{
        group::Group,
        host::Host,
        image::{Image, ImagePartition},
        task::{Task, TaskState, TaskType},
    },
    registry::HostConnectionEvent,
};

impl From<pb::Host> for Host {
    fn from(value: pb::Host) -> Self {
        Self::new(
            value.id,
            value.name,
            value.mac_address,
            value.disk_size_bytes,
        )
    }
}

impl From<Host> for pb::Host {
    fn from(value: Host) -> Self {
        Self {
            id: value.id,
            mac_address: value.mac_address,
            name: value.name,
            disk_size_bytes: value.disk_size,
        }
    }
}

impl From<HostConnectionEvent> for pb::HostConnectionEvent {
    fn from(value: HostConnectionEvent) -> Self {
        Self {
            id: value.id,
            connected: value.connected,
        }
    }
}

impl From<Image> for pb::Image {
    fn from(value: Image) -> Self {
        pb::Image {
            id: value.id,
            name: value.name,
            captured_at: value.captured_at.map(|v| SystemTime::from(v).into()),
            status: value.status.to_string(),
            error_message: value.error,
            partitions: value.partitions.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ImagePartition> for pb::ImagePartition {
    fn from(value: ImagePartition) -> Self {
        Self {
            id: value.id,
            partition_number: value.partition_number,
            fstype: value.fstype,
            size_bytes: value.size_bytes,
        }
    }
}

impl From<Task> for pb::Task {
    fn from(value: Task) -> Self {
        Self {
            id: value.id,
            r#type: value.task_type.into(),
            hosts: value.hosts,
            image_id: value.image_id,
            state: value.state.into(),
            created_at: Some(Timestamp::from(SystemTime::from(value.created_at))),
            started_at: value
                .started_at
                .map(|v| Timestamp::from(SystemTime::from(v))),
            finished_at: value
                .finished_at
                .map(|v| Timestamp::from(SystemTime::from(v))),
            error: value.error,
        }
    }
}

impl From<TaskType> for i32 {
    fn from(value: TaskType) -> Self {
        match value {
            TaskType::Capture => pb::TaskType::TypeCapture.into(),
            TaskType::Deploy => pb::TaskType::TypeDeploy.into(),
            TaskType::Multicast => pb::TaskType::TypeMulticast.into(),
        }
    }
}

impl From<TaskState> for i32 {
    fn from(value: TaskState) -> Self {
        let state = match value {
            TaskState::Pending => pb::TaskState::TaskPending,
            TaskState::Running => pb::TaskState::TaskRunning,
            TaskState::Done => pb::TaskState::TaskDone,
            TaskState::Failed => pb::TaskState::TaskFailed,
            TaskState::Cancelled => pb::TaskState::TaskCancelled,
        };
        state.into()
    }
}

impl From<Group> for pb::Group {
    fn from(value: Group) -> Self {
        Self {
            id: value.id,
            name: value.name,
        }
    }
}
