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

use crate::{
    domain::{image::ImageRepository, task::TaskRepository},
    service::image::ImageService,
};

struct RunningMulticastTask {
    id: i64,
    handle: JoinHandle<()>,
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
        for task in task_repo
            .get_all()
            .await?
            .iter()
            .filter(|t| t.task_type == TaskType::Multicast && t.state == TaskState::Running)
        {
            task_repo.mark_failed(task.id, &error).await?;
        }
        if let Some(task) = task_repo.get_next_multicast().await? {
            new.notify_new(task.id)?;
        }
        Ok(new)
    }

    pub fn get_running_task(&self) -> Option<i64> {
        self.current.lock().unwrap().as_ref().map(|r| r.id)
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
        });

        Ok(())
    }

    async fn handle_loop(&self) -> Result {
        while let Some(t) = self.task_repo.get_next_multicast().await? {
            // this is save, since the lock in notify_handler is not released until the
            // handle is set
            {
                let mut lock = self.current.lock().unwrap();
                if let Some(r) = lock.as_mut() {
                    r.id = t.id;
                }
            }
            if let Err(e) = self.do_work(t.clone()).await {
                let _ = self.task_repo.mark_failed(t.id, &e.to_string()).await;
                error!(err=%e, "task failed: {:?}", t)
            } else {
                let _ = self.task_repo.mark_finished(t.id).await;
                debug!(id=%t.id, "multicast task finished");
            }
        }

        Ok(())
    }

    async fn do_work(&self, task: Task) -> Result {
        if task.task_type != TaskType::Multicast || task.state != TaskState::Pending {
            return Err(AppError::FailedPrecondition(
                "tasktype or state is wrong".to_string(),
            ));
        }
        let Some(image_id) = task.image_id else {
            return Err(AppError::FailedPrecondition(
                "image id is not set".to_string(),
            ));
        };
        self.task_repo.start(task.id).await?;

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
