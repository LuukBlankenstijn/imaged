pub mod capture;
pub mod deploy;
pub mod multicast;
pub mod reboot;

pub use capture::CaptureTask;
pub use deploy::DeployTask;
pub use multicast::MulticastTask;
pub use reboot::RebootTask;

use std::time::Duration;

use derive_more::Display;
use enum_dispatch::enum_dispatch;
use imaged_disk::Progress;

use crate::agent::Agent;
use crate::server::Rejected;
use crate::status::{Outcome, PartitionProgress, Phase, Step};
use crate::sys::disk::{BlockDevice, PartitionTarget};

const PARTCLONE_LOG: &str = "/tmp/partclone-log";
const REPORT_RETRY_START: Duration = Duration::from_secs(1);
const REPORT_RETRY_CAP: Duration = Duration::from_secs(30);

#[allow(
    async_fn_in_trait,
    reason = "only awaited through concrete task types (the Task enum and test doubles), whose futures are Send"
)]
#[enum_dispatch]
pub trait ClientTaskExt: std::fmt::Display {
    fn task_id(&self) -> i64;

    fn agent(&self) -> &Agent;

    async fn run(&self) -> anyhow::Result<()> {
        image_target_disk(self).await
    }

    async fn target_disk(&self) -> anyhow::Result<BlockDevice> {
        crate::sys::disk::find_target_disk().await
    }

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
        report(self, || {
            self.agent().server.mark_failed(self.task_id(), err)
        })
        .await?;
        self.agent()
            .status
            .task_ended(Outcome::Failed(err.to_string()));
        Ok(())
    }
}

async fn image_target_disk(task: &(impl ClientTaskExt + ?Sized)) -> anyhow::Result<()> {
    let status = &task.agent().status;
    let disk = task.target_disk().await?;

    status.update_task(|task| task.step = Step::PartitionTable);
    task.handle_partition_table(&disk.get_device()).await?;

    let disk = task.target_disk().await?;

    let partitions = task.plan_partitions(&disk).await?;
    let progress = status.partitions(&partitions);
    for (index, (partition, progress)) in partitions.iter().zip(progress).enumerate() {
        progress.report(Progress::default());
        task.handle_partition(partition.clone(), progress).await?;
        status.update_task(|task| task.partitions[index].phase = Phase::Done);
    }

    status.update_task(|task| task.step = Step::Finishing);
    task.after_partitions(&partitions).await
}

async fn report<Report>(
    task: &(impl ClientTaskExt + ?Sized),
    send: impl Fn() -> Report,
) -> anyhow::Result<()>
where
    Report: Future<Output = anyhow::Result<()>>,
{
    let mut delay = REPORT_RETRY_START;
    loop {
        match send().await {
            Ok(()) => return Ok(()),
            Err(e) if Rejected::is_final(&e) => return Err(e),
            Err(e) => {
                tracing::warn!(task=%task, error=%format_args!("{e:#}"), retry_in=?delay, "could not report the result, retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(REPORT_RETRY_CAP);
            }
        }
    }
}

async fn report_finished(task: &(impl ClientTaskExt + ?Sized)) -> anyhow::Result<()> {
    report(task, || task.agent().server.mark_finished(task.task_id())).await?;
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
    let layout = agent.server.image_layout(task_id).await?;
    if layout.partitions.is_empty() {
        anyhow::bail!("image for task {task_id} has no partitions to restore");
    }
    layout
        .partitions
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
