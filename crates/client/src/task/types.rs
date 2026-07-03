use std::sync::Arc;

use anyhow::Result;
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::task::implementations::ClientTaskExt;
use crate::transport::ApiClient;

pub trait RunnableClientTask: ClientTaskExt {
    async fn run(&self, state: Arc<ClientState>, cancel: CancellationToken) -> Result<()> {
        let disk = crate::sys::disk::find_target_disk().await?;
        let device = format!("/dev/{}", disk.name);

        self.handle_partition_table(&state.http, &device).await?;
        if cancel.is_cancelled() {
            anyhow::bail!("cancelled");
        }

        // update all partitions just in case the partitions were changed
        let disk = crate::sys::disk::find_target_disk().await?;

        for partition in disk.children.into_iter() {
            self.handle_partition(&state.http, partition).await?;
            if cancel.is_cancelled() {
                anyhow::bail!("cancelled");
            }
        }

        Ok(())
    }
}
impl<T: ClientTaskExt + ?Sized> RunnableClientTask for T {}

pub struct ClientState {
    pub http: crate::transport::ApiClient,
    pub current_task: Mutex<Option<RunningTask>>,
}

impl ClientState {
    pub fn new(base_url: String, mac: String) -> Result<Self> {
        Ok(Self {
            http: ApiClient::new(base_url, mac)?,
            current_task: Mutex::new(None),
        })
    }

    pub fn client(&self) -> &ApiClient {
        &self.http
    }
}

pub struct RunningTask {
    pub task_id: i64,
    // can be used for forcefully aborting the task
    pub _handle: JoinHandle<()>,
    pub cancel: tokio_util::sync::CancellationToken,
}
