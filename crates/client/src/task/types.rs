use anyhow::Result;
use tokio::{sync::Mutex, task::JoinHandle};

use crate::task::implementations::ClientTaskExt;

pub trait RunnableClientTask: ClientTaskExt {
    async fn run(&self) -> Result<()> {
        let disk = crate::sys::disk::find_target_disk().await?;
        let device = format!("/dev/{}", disk.name);

        self.handle_partition_table(&device).await?;

        let disk = crate::sys::disk::find_target_disk().await?;

        for partition in self.plan_partitions(&disk).await? {
            self.handle_partition(partition).await?;
        }

        Ok(())
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
