pub mod capture;
pub mod deploy;
pub mod multicast;
pub mod reboot;

pub use capture::CaptureTask;
pub use deploy::DeployTask;
pub use multicast::MulticastTask;
pub use reboot::RebootTask;

use derive_more::Display;
use enum_dispatch::enum_dispatch;

use crate::sys::disk::{BlockDevice, PartitionTarget};

#[allow(
    async_fn_in_trait,
    reason = "trait is internal to client, only driven on a single-threaded executor"
)]
#[enum_dispatch]
pub trait ClientTaskExt: std::fmt::Display {
    async fn handle_partition_table(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn plan_partitions(&self, disk: &BlockDevice) -> anyhow::Result<Vec<PartitionTarget>> {
        disk.formatted_partitions()
    }

    async fn handle_partition(&self, _: PartitionTarget) -> anyhow::Result<()> {
        Ok(())
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        tracing::info!(task=%self, "finished task successfully");
        Ok(())
    }

    async fn finalize_error(&self, err: &str) -> anyhow::Result<()> {
        tracing::error!(task=%self, error=%err, "did not finish task successfully");
        Ok(())
    }
}

async fn image_partitions(
    task_id: i64,
    disk: &BlockDevice,
) -> anyhow::Result<Vec<PartitionTarget>> {
    let partitions = api::deploy::download_partitions(task_id).await?;
    if partitions.is_empty() {
        anyhow::bail!("image for task {task_id} has no partitions to restore");
    }
    partitions
        .into_iter()
        .map(|p| disk.partition_target(p.partition_number, p.fstype))
        .collect()
}

#[derive(Display)]
#[display("{_variant}")]
#[enum_dispatch(ClientTaskExt)]
pub enum Task {
    Capture(capture::CaptureTask),
    Deploy(deploy::DeployTask),
    Multicast(multicast::MulticastTask),
    Reboot(reboot::RebootTask),
}

impl From<imaged_shared::Task> for Task {
    fn from(value: imaged_shared::Task) -> Self {
        match value.task_type {
            imaged_shared::TaskType::Capture => Self::Capture(capture::CaptureTask::new(value.id)),
            imaged_shared::TaskType::Deploy => Self::Deploy(deploy::DeployTask::new(value.id)),
            imaged_shared::TaskType::Multicast => {
                Self::Multicast(multicast::MulticastTask::new(value.id))
            }
            imaged_shared::TaskType::Reboot => Self::Reboot(reboot::RebootTask::new(value.id)),
        }
    }
}
