use std::time::SystemTime;

use imaged_rpc::dashboard::v1::{self as pb};

use crate::{
    domain::{
        host::Host,
        image::{Image, ImagePartition},
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
        let (status, error_message) = value.status.into_parts();
        pb::Image {
            id: value.id,
            name: value.name,
            captured_at: value.captured_at.map(|v| SystemTime::from(v).into()),
            status,
            error_message,
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
