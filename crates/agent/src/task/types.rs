use anyhow::Result;
use imaged_disk::Progress;
use tokio::{sync::Mutex, task::JoinHandle};

use crate::task::implementations::ClientTaskExt;

#[allow(
    async_fn_in_trait,
    reason = "only awaited through concrete task types (the Task enum and test doubles), whose futures are Send"
)]
pub trait RunnableClientTask: ClientTaskExt {
    async fn run(&self) -> Result<()> {
        let progress = self.server().progress(self.task_id());
        let disk = crate::sys::disk::find_target_disk().await?;
        let device = format!("/dev/{}", disk.name);

        progress.partition_table();
        self.handle_partition_table(&device).await?;

        let disk = crate::sys::disk::find_target_disk().await?;

        let partitions = self.plan_partitions(&disk).await?;
        for (partition, reporter) in partitions.iter().zip(progress.partitions(&partitions)) {
            reporter.report(Progress::default());
            self.handle_partition(partition.clone(), reporter).await?;
        }

        progress.finishing();
        self.after_partitions(&partitions).await
    }
}
impl<T: ClientTaskExt + ?Sized> RunnableClientTask for T {}

#[derive(Default)]
pub struct ClientState {
    pub current_task: Mutex<Option<RunningTask>>,
}

pub struct RunningTask {
    pub task_id: i64,
    // can be used for forcefully aborting the task
    pub _handle: JoinHandle<()>,
    pub cancel: tokio_util::sync::CancellationToken,
}
