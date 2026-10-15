use anyhow::Result;
use imaged_disk::Progress;
use tokio::{sync::Mutex, task::JoinHandle};

use crate::status::{Phase, Step};
use crate::task::implementations::ClientTaskExt;

#[allow(
    async_fn_in_trait,
    reason = "only awaited through concrete task types (the Task enum and test doubles), whose futures are Send"
)]
pub trait RunnableClientTask: ClientTaskExt {
    async fn run(&self) -> Result<()> {
        let status = &self.agent().status;
        let disk = crate::sys::disk::find_target_disk().await?;
        let device = format!("/dev/{}", disk.name);

        status.update_task(|task| task.step = Step::PartitionTable);
        self.handle_partition_table(&device).await?;

        let disk = crate::sys::disk::find_target_disk().await?;

        let partitions = self.plan_partitions(&disk).await?;
        let progress = status.partitions(&partitions);
        for (index, (partition, progress)) in partitions.iter().zip(progress).enumerate() {
            progress.report(Progress::default());
            self.handle_partition(partition.clone(), progress).await?;
            status.update_task(|task| task.partitions[index].phase = Phase::Done);
        }

        status.update_task(|task| task.step = Step::Finishing);
        self.after_partitions(&partitions).await
    }
}
impl<T: ClientTaskExt + ?Sized> RunnableClientTask for T {}

#[derive(Default)]
pub struct ClientState {
    pub current_task: Mutex<Option<RunningTask>>,
}

impl ClientState {
    pub async fn cancel(&self, task_id: i64) {
        let current = self.current_task.lock().await;
        if let Some(running) = current.as_ref()
            && running.task_id == task_id
        {
            running.cancel.cancel();
        }
    }
}

pub struct RunningTask {
    pub task_id: i64,
    // can be used for forcefully aborting the task
    pub _handle: JoinHandle<()>,
    pub cancel: tokio_util::sync::CancellationToken,
}
