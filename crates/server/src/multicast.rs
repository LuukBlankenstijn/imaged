use crate::{
    domain::task::{Task, TaskState, TaskType},
    error::{AppError, Result},
};
use imaged_shared::{MULTICAST_DATA_ADDRESS, MULTICAST_RVD_ADDRESS, get_multicast_port};
use std::{
    process::Stdio,
    sync::{Arc, Mutex},
};
use tracing::{debug, error, info};

use tokio::{process::Command, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    domain::{image::ImageRepository, task::TaskRepository},
    service::image::ImageService,
};

struct RunningMulticastTask {
    id: i64,
    handle: JoinHandle<()>,
    /// Cancels the in-flight `do_work` for `id`, killing the udp-sender child.
    cancel: CancellationToken,
}

struct SlotGuard(Arc<Mutex<Option<RunningMulticastTask>>>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut slot = match self.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = None;
    }
}

#[derive(Clone)]
pub struct MulticastManager {
    task_repo: Arc<dyn TaskRepository>,
    image_repo: Arc<dyn ImageRepository>,
    image_service: Arc<ImageService>,
    current: Arc<Mutex<Option<RunningMulticastTask>>>,
}

impl MulticastManager {
    pub async fn new(
        task_repo: Arc<dyn TaskRepository>,
        image_repo: Arc<dyn ImageRepository>,
        image_service: Arc<ImageService>,
    ) -> Result<Self> {
        let new = Self {
            task_repo: task_repo.clone(),
            image_repo,
            image_service,
            current: Arc::new(Mutex::new(None)),
        };
        // mark all tasks that are already started as error
        let error = String::from("Server stopped while task was running");
        for task in task_repo.get_all().await?.iter().filter(|t| {
            t.task_type == TaskType::Multicast && t.aggregate_state() == TaskState::Running
        }) {
            task_repo.mark_all_failed(task.id, &error).await?;
        }
        if let Some(task) = task_repo.get_next_multicast().await? {
            new.notify_new(task.id)?;
        }
        Ok(new)
    }

    pub fn notify_new(&self, task_id: i64) -> Result {
        let mut lock = self.current.lock().unwrap();

        if lock
            .as_ref()
            .map(|r| r.handle.is_finished())
            .unwrap_or(false)
        {
            *lock = None;
        }

        if lock.is_some() {
            return Ok(());
        }

        let self_clone = self.clone();
        let handle = tokio::spawn(async move {
            let _guard = SlotGuard(self_clone.current.clone());
            if let Err(e) = self_clone.handle_loop().await {
                error!(err=%e, "multicast task handler failed");
            }
        });

        *lock = Some(RunningMulticastTask {
            handle,
            // this id can be incorrect since the loop fetches the task for himself
            id: task_id,
            cancel: CancellationToken::new(),
        });

        Ok(())
    }

    /// Cancel the in-flight multicast send if `task_id` is the one currently
    /// running. The caller is responsible for marking the task cancelled in the
    /// DB first; this stops the detached sender loop's current `do_work` (and
    /// its udp-sender child) so the loop moves on to the next queued task.
    pub fn cancel(&self, task_id: i64) {
        let lock = self.current.lock().unwrap();
        if let Some(r) = lock.as_ref()
            && r.id == task_id
        {
            r.cancel.cancel();
            debug!(task_id, "cancelling running multicast task");
        }
    }

    async fn handle_loop(&self) -> Result {
        while let Some(t) = self.task_repo.get_next_multicast().await? {
            // Fresh token for this task, published into the slot so `cancel`
            // can stop it. This is safe since the lock in notify_new is not
            // released until the handle is set.
            let cancel = CancellationToken::new();
            {
                let mut lock = self.current.lock().unwrap();
                if let Some(r) = lock.as_mut() {
                    r.id = t.id;
                    r.cancel = cancel.clone();
                }
            }

            tokio::select! {
                biased;
                // Cancel wins: dropping the do_work future drops the udp-sender
                // Child, which is killed via kill_on_drop. The DB rows are
                // already marked cancelled by the caller, so nothing to do here.
                _ = cancel.cancelled() => {
                    debug!(id=%t.id, "multicast task cancelled");
                }
                res = self.do_work(t.clone()) => {
                    if let Err(e) = res {
                        let _ = self.task_repo.mark_all_failed(t.id, &e.to_string()).await;
                        error!(err=%e, "task failed: {:?}", t)
                    } else {
                        let _ = self.task_repo.mark_all_finished(t.id).await;
                        debug!(id=%t.id, "multicast task finished");
                    }
                }
            }
        }

        Ok(())
    }

    async fn do_work(&self, task: Task) -> Result {
        // Re-fetch: `task` is a snapshot from get_next_multicast and may have
        // been cancelled in the tiny window before this iteration published its
        // cancellation token. Trust the DB so a cancel that raced the dequeue
        // still prevents the send.
        let task = self.task_repo.get(task.id).await?;
        if task.task_type != TaskType::Multicast || task.aggregate_state() != TaskState::Pending {
            return Err(AppError::FailedPrecondition(
                "tasktype or state is wrong".to_string(),
            ));
        }
        let Some(image_id) = task.image_id else {
            return Err(AppError::FailedPrecondition(
                "image id is not set".to_string(),
            ));
        };
        for h in &task.hosts {
            self.task_repo.start(task.id, h.host_id).await?;
        }

        let num_receivers = task.hosts.len();

        let partition_table_path = self.image_service.get_partition_table_path(image_id);

        upd_sender(&partition_table_path, get_multicast_port(0), num_receivers).await?;

        let partitions = self.image_repo.get_partitions(image_id).await?;
        for p in partitions.into_iter() {
            info!(task_id=%task.id, image_id=%image_id, partition_number=%p.partition_number, "sending partition over multicast");
            let partition_path = self
                .image_service
                .get_partition_path(image_id, p.partition_number);
            upd_sender(
                &partition_path,
                get_multicast_port(p.partition_number),
                num_receivers,
            )
            .await?;
        }

        Ok(())
    }
}

async fn upd_sender(file: &str, portbase: u16, num_receivers: usize) -> Result {
    let rvd_address = MULTICAST_RVD_ADDRESS;
    let data_address = MULTICAST_DATA_ADDRESS;
    let interface = std::env::var("MULTICAST_INTERFACE").unwrap_or("lo".to_string());
    let status = Command::new("udp-sender")
        .args([
            "--file",
            file,
            "--portbase",
            &portbase.to_string(),
            "--mcast-rdv-address",
            rvd_address,
            "--mcast-data-address",
            data_address,
            "--min-receivers",
            &num_receivers.to_string(),
            "--nokbd",
            "--autorate",
            "--interface",
            &interface,
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Internal(format!("failed to spawn udp-sender {e}")))?
        .wait()
        .await
        .map_err(|_| AppError::Internal("udp-sender process failed to wait".to_string()))?;

    if !status.success() {
        return Err(AppError::Internal(
            "udp-sender exited unsuccessfully".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// Spawns a child that only creates `marker` if it survives ~1s, mirroring
    /// how `upd_sender` runs udp-sender: `kill_on_drop(true)` + `spawn().wait()`.
    async fn run_child(marker: &std::path::Path) {
        let _ = Command::new("sh")
            .arg("-c")
            .arg(format!("sleep 1 && touch {}", marker.display()))
            .kill_on_drop(true)
            .spawn()
            .expect("spawn sh")
            .wait()
            .await;
    }

    /// Regression test for the multicast cancel bug: cancelling a running send
    /// must actually kill the sender child, not just flip the database. We drive
    /// the same `select!(token.cancelled(), do_work)` shape as `handle_loop` and
    /// prove the child is killed before it can finish its work.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_kills_running_child() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let marker =
            std::env::temp_dir().join(format!("imaged-cancel-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_file(&marker);

        let token = CancellationToken::new();
        let trigger = token.clone();
        // Cancel once the child has spawned and is mid-sleep. Uses a plain thread
        // so we don't depend on tokio's "time" feature.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            trigger.cancel();
        });

        tokio::select! {
            biased;
            _ = token.cancelled() => {}
            _ = run_child(&marker) => panic!("child completed instead of being cancelled"),
        }

        // Absent the kill, the child would `touch` the marker at ~1s. Wait past
        // that and confirm it never happened.
        std::thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "child survived cancellation and created {}",
            marker.display()
        );
        let _ = std::fs::remove_file(&marker);
    }
}
