mod capture;
mod deploy;
mod multicast;

use derive_more::Display;
use enum_dispatch::enum_dispatch;

use crate::{sys::disk::BlockDevice, transport::ApiClient};

#[enum_dispatch]
pub trait ClientTaskExt: std::fmt::Display {
    async fn handle_partition_table(&self, api: &ApiClient, device: &str) -> anyhow::Result<()>;

    async fn handle_partition(&self, api: &ApiClient, partition: BlockDevice)
    -> anyhow::Result<()>;

    async fn finalize(&self, _: &ApiClient) -> anyhow::Result<()> {
        tracing::info!(task=%self, "finished task successfully");
        Ok(())
    }

    async fn finalize_error(&self, _: &ApiClient, err: &str) -> anyhow::Result<()> {
        tracing::error!(task=%self, error=%err, "did not finish task successfully");
        Ok(())
    }
}

#[derive(Display)]
#[display("{_variant}")]
#[enum_dispatch(ClientTaskExt)]
pub enum Task {
    Capture(capture::CaptureTask),
    Deploy(deploy::DeployTask),
    Multicast(multicast::MulticastTask),
}

impl From<imaged_shared::Task> for Task {
    fn from(value: imaged_shared::Task) -> Self {
        match value.task_type {
            imaged_shared::TaskType::Capture => Self::Capture(capture::CaptureTask::new(value.id)),
            imaged_shared::TaskType::Deploy => Self::Deploy(deploy::DeployTask::new(value.id)),
            imaged_shared::TaskType::Multicast => Self::Multicast(multicast::MulticastTask),
        }
    }
}
