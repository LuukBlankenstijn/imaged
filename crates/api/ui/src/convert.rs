use imaged_core::domain::{
    group::Group as DGroup,
    host::Host as DHost,
    image::{Image as DImage, ImagePartition as DPartition, ImageStatus as DStatus},
    task::{Task as DTask, TaskHost as DTaskHost, TaskState as DState, TaskType as DType},
};
use imaged_core::multicast::MulticastProgress as DProgress;
use imaged_core::registry::{ConnectionChange as DChange, HostConnectionEvent as DConn};

use crate::model;

impl From<DHost> for model::Host {
    fn from(h: DHost) -> Self {
        Self {
            id: h.id,
            mac_address: h.mac_address,
            name: h.name,
            disk_size_bytes: h.disk_size,
            ip: h.ip,
        }
    }
}

impl From<DConn> for model::HostConnectionEvent {
    fn from(e: DConn) -> Self {
        Self {
            id: e.id,
            connected: e.connected,
        }
    }
}

impl From<DChange> for model::ConnectionUpdate {
    fn from(c: DChange) -> Self {
        match c {
            DChange::Connected(hosts) => Self::Connected(hosts),
            DChange::Changed(event) => Self::Changed(event.into()),
        }
    }
}

impl From<DProgress> for model::MulticastProgress {
    fn from(p: DProgress) -> Self {
        Self {
            task_id: p.task_id,
            fraction: p.fraction,
            bytes_per_second: p.bytes_per_second,
            receivers: p.receivers,
            step: p.step,
            steps: p.steps,
            eta_seconds: p.eta.map(|eta| eta.as_secs()),
        }
    }
}

impl From<DStatus> for model::ImageStatus {
    fn from(s: DStatus) -> Self {
        match s {
            DStatus::Empty => Self::Empty,
            DStatus::Capturing => Self::Capturing,
            DStatus::Ready => Self::Ready,
            DStatus::Faulted => Self::Faulted,
        }
    }
}

impl From<DPartition> for model::ImagePartition {
    fn from(p: DPartition) -> Self {
        Self {
            id: p.id,
            partition_number: p.partition_number,
            fstype: p.fstype,
            size_bytes: p.size_bytes,
        }
    }
}

impl From<DImage> for model::Image {
    fn from(i: DImage) -> Self {
        Self {
            id: i.id,
            name: i.name,
            captured_at: i.captured_at.map(|t| t.timestamp_millis()),
            status: i.status.into(),
            error_message: i.error,
            partitions: i.partitions.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<DType> for model::TaskType {
    fn from(t: DType) -> Self {
        match t {
            DType::Capture => Self::Capture,
            DType::Deploy => Self::Deploy,
            DType::Multicast => Self::Multicast,
            DType::Reboot => Self::Reboot,
        }
    }
}

impl From<DState> for model::TaskState {
    fn from(s: DState) -> Self {
        match s {
            DState::Pending => Self::Pending,
            DState::Running => Self::Running,
            DState::Done => Self::Done,
            DState::Cancelled => Self::Cancelled,
            DState::Failed => Self::Failed,
            DState::Partial => Self::Partial,
        }
    }
}

impl From<DTaskHost> for model::TaskHost {
    fn from(h: DTaskHost) -> Self {
        Self {
            host_id: h.host_id,
            state: h.state.into(),
            error: h.error,
            started_at: h.started_at.map(|t| t.timestamp_millis()),
            finished_at: h.finished_at.map(|t| t.timestamp_millis()),
        }
    }
}

impl From<DTask> for model::Task {
    fn from(t: DTask) -> Self {
        let state = t.aggregate_state().into();
        Self {
            id: t.id,
            r#type: t.task_type.into(),
            state,
            hosts: t.hosts.into_iter().map(Into::into).collect(),
            image_id: t.image_id,
            created_at: t.created_at.timestamp_millis(),
            image_name: t.image_name,
            image_deleted: t.image_deleted,
        }
    }
}

impl From<DGroup> for model::Group {
    fn from(g: DGroup) -> Self {
        Self {
            id: g.id,
            name: g.name,
        }
    }
}
