mod capture;
mod deploy;
mod multicast;
mod reboot;

use derive_more::Display;
use enum_dispatch::enum_dispatch;

use crate::{sys::disk::BlockDevice, transport::ApiClient};

#[enum_dispatch]
pub trait ClientTaskExt: std::fmt::Display {
    async fn handle_partition_table(&self, _: &ApiClient, _: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn handle_partition(&self, _: &ApiClient, _: BlockDevice) -> anyhow::Result<()> {
        Ok(())
    }

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
    Reboot(reboot::RebootTask),
}

impl From<imaged_shared::Task> for Task {
    fn from(value: imaged_shared::Task) -> Self {
        match value.task_type {
            imaged_shared::TaskType::Capture => Self::Capture(capture::CaptureTask::new(value.id)),
            imaged_shared::TaskType::Deploy => Self::Deploy(deploy::DeployTask::new(value.id)),
            imaged_shared::TaskType::Multicast => Self::Multicast(multicast::MulticastTask),
            imaged_shared::TaskType::Reboot => Self::Reboot(reboot::RebootTask::new(value.id)),
        }
    }
}
