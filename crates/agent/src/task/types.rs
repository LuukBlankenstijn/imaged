use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

#[derive(Default)]
pub struct ClientState {
    pub current_task: Mutex<Option<RunningTask>>,
    idle: watch::Sender<()>,
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

    pub fn idle(&self) -> watch::Receiver<()> {
        self.idle.subscribe()
    }

    pub(crate) fn became_idle(&self) {
        self.idle.send_replace(());
    }
}

pub struct RunningTask {
    pub task_id: i64,
    // can be used for forcefully aborting the task
    pub _handle: JoinHandle<()>,
    pub cancel: tokio_util::sync::CancellationToken,
}
