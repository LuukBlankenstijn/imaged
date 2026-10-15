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

use crate::agent::Agent;
use crate::status::{Outcome, PartitionProgress};
use crate::sys::disk::{BlockDevice, PartitionTarget};

const PARTCLONE_LOG: &str = "/tmp/partclone-log";

#[allow(
    async_fn_in_trait,
    reason = "only awaited through concrete task types (the Task enum and test doubles), whose futures are Send"
)]
#[enum_dispatch]
pub trait ClientTaskExt: std::fmt::Display {
    fn task_id(&self) -> i64;

    fn agent(&self) -> &Agent;

    async fn handle_partition_table(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn plan_partitions(&self, disk: &BlockDevice) -> anyhow::Result<Vec<PartitionTarget>> {
        disk.formatted_partitions()
    }

    async fn handle_partition(
        &self,
        _: PartitionTarget,
        _: PartitionProgress,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn after_partitions(&self, _: &[PartitionTarget]) -> anyhow::Result<()> {
        Ok(())
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        report_finished(self).await
    }

    async fn finalize_error(&self, err: &str) -> anyhow::Result<()> {
        tracing::error!(task=%self, error=%err, "did not finish task successfully");
        self.agent()
            .status
            .task_ended(Outcome::Failed(err.to_string()));
        self.agent().server.mark_failed(self.task_id(), err).await
    }
}

async fn report_finished(task: &(impl ClientTaskExt + ?Sized)) -> anyhow::Result<()> {
    task.agent().server.mark_finished(task.task_id()).await?;
    task.agent().status.task_ended(Outcome::Done);
    tracing::info!(task=%task, "finished task successfully");
    Ok(())
}

async fn finish_and_reboot(task: &impl ClientTaskExt) -> anyhow::Result<()> {
    report_finished(task).await?;
    crate::reboot::after_countdown(task.agent()).await
}

async fn image_partitions(
    agent: &Agent,
    task_id: i64,
    disk: &BlockDevice,
) -> anyhow::Result<Vec<PartitionTarget>> {
    let partitions = agent.server.partitions(task_id).await?;
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

impl Task {
    pub fn new(agent: Agent, task: &imaged_model::Task) -> Self {
        let id = task.id;
        match task.task_type {
            imaged_model::TaskType::Capture => Self::Capture(CaptureTask::new(agent, id)),
            imaged_model::TaskType::Deploy => Self::Deploy(DeployTask::new(agent, id)),
            imaged_model::TaskType::Multicast => Self::Multicast(MulticastTask::new(agent, id)),
            imaged_model::TaskType::Reboot => Self::Reboot(RebootTask::new(agent, id)),
        }
    }
}
