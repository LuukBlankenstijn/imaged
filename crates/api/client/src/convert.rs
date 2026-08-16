use imaged_core::domain::image::ImagePartition as DPartition;

use crate::model;

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
