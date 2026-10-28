use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};

use futures::FutureExt;
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;
use tracing::error;

use crate::{
    domain::{
        host::HostRepository,
        image::ImageRepository,
        task::{Task, TaskRepository, TaskType},
    },
    progress::ProgressBoard,
    registry::HostRegistry,
    service::image::ImageService,
};
use imaged_model::{
    error::{AppError, Result},
    multicast::{ReceiverProgress, TransferPhase},
    task::TaskState,
};

#[cfg(target_os = "linux")]
mod session;

const DB_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const ORPHANED: &str = "Server stopped while task was running";
const DISCONNECTED: &str = "Host disconnected before reporting the multicast result";

#[derive(Clone, Debug, PartialEq)]
pub struct MulticastProgress {
    pub task_id: i64,
    pub phase: TransferPhase,
    pub bytes_per_second: f64,
    pub step: usize,
    pub steps: usize,
    pub eta: Option<Duration>,
    pub receivers: Vec<ReceiverProgress>,
}

pub struct MulticastManager {
    wake: watch::Sender<()>,
    _worker: AbortOnDropHandle<()>,
}

impl MulticastManager {
    pub async fn start(
        tasks: Arc<dyn TaskRepository>,
        hosts: Arc<dyn HostRepository>,
        images: Arc<dyn ImageRepository>,
        image_service: Arc<ImageService>,
        registry: Arc<HostRegistry>,
        progress: Arc<ProgressBoard>,
        interface: String,
    ) -> Result<Self> {
        let worker = Worker {
            tasks,
            hosts,
            images,
            image_service,
            registry,
            progress,
            interface,
        };
        worker.fail_orphans().await?;
        let (wake, mut woken) = watch::channel(());
        if worker.tasks.get_next_multicast().await?.is_some() {
            woken.mark_changed();
        }
        Ok(Self {
            wake,
            _worker: AbortOnDropHandle::new(tokio::spawn(worker.run(woken))),
        })
    }

    pub fn wake(&self) {
        self.wake.send_replace(());
    }
}

#[cfg_attr(not(target_os = "linux"), expect(dead_code))]
struct Worker {
    tasks: Arc<dyn TaskRepository>,
    hosts: Arc<dyn HostRepository>,
    images: Arc<dyn ImageRepository>,
    image_service: Arc<ImageService>,
    registry: Arc<HostRegistry>,
    progress: Arc<ProgressBoard>,
    interface: String,
}

impl Worker {
    async fn fail_orphans(&self) -> Result {
        for task_type in [TaskType::Multicast, TaskType::MulticastPartition] {
            self.tasks.fail_running(task_type, ORPHANED).await?;
        }
        Ok(())
    }

    async fn run(self, mut woken: watch::Receiver<()>) {
        while woken.changed().await.is_ok() {
            while let Some(task) = retrying(|| self.tasks.get_next_multicast()).await {
                self.run_session(task, &mut woken).await;
            }
        }
    }

    async fn run_session(&self, task: Task, woken: &mut watch::Receiver<()>) {
        retrying(|| self.claim(&task)).await;
        let outcome = AssertUnwindSafe(self.session(&task, woken))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(AppError::Internal("multicast session panicked".to_string())));
        self.progress.end_multicast();
        if let Err(e) = &outcome {
            error!(task_id = task.id, err = %e, "multicast session failed");
        }
        retrying(|| self.settle(task.id, outcome.as_ref().err())).await;
    }

    async fn claim(&self, task: &Task) -> Result {
        for host in task.hosts_in(TaskState::Pending) {
            match self.tasks.get_next(host).await? {
                Some(next) if next.id != task.id => {
                    let busy = format!("Host was busy with task {}", next.id);
                    self.tasks.mark_failed(task.id, host, &busy).await?
                }
                _ => self.tasks.start(task.id, host).await?,
            }
        }
        Ok(())
    }

    async fn settle(&self, task_id: i64, failure: Option<&AppError>) -> Result {
        let connected = self.registry.connected_hosts();
        for host in self.running(task_id).await? {
            if let Some(e) = failure {
                self.tasks
                    .mark_failed(task_id, host, &e.to_string())
                    .await?;
                self.registry.dispatch(host);
            } else if !connected.contains(&host) {
                self.tasks.mark_failed(task_id, host, DISCONNECTED).await?;
            }
        }
        Ok(())
    }

    async fn running(&self, task_id: i64) -> Result<Vec<i64>> {
        Ok(self
            .tasks
            .get(task_id)
            .await?
            .hosts_in(TaskState::Running)
            .collect())
    }
}

#[cfg(not(target_os = "linux"))]
impl Worker {
    async fn session(&self, _task: &Task, _woken: &mut watch::Receiver<()>) -> Result {
        Err(AppError::FailedPrecondition(
            "multicast is only supported on Linux".to_string(),
        ))
    }
}

async fn retrying<T, F>(mut attempt: impl FnMut() -> F) -> T
where
    F: Future<Output = Result<T>>,
{
    loop {
        match attempt().await {
            Ok(value) => return value,
            Err(e) => {
                error!(err = %e, "multicast bookkeeping failed, retrying");
                tokio::time::sleep(DB_RETRY_INTERVAL).await;
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::di::DIContainer;
    use crate::progress::Snapshot;
    use crate::test_support::container;
    use imaged_model::image::ImageKind;
    use imaged_model::{get_multicast_port, multicast_group};
    use scuttlecast::{error::ProtoError, receiver::Receiver};
    use std::{
        net::Ipv4Addr,
        path::PathBuf,
        sync::{
            Mutex, PoisonError,
            atomic::{AtomicU64, Ordering},
        },
    };
    use tokio::{task::JoinHandle, time::Instant};

    static SESSIONS: Mutex<()> = Mutex::new(());
    static IMAGE_ID: AtomicU64 = AtomicU64::new(0);
    const SETTLE: Duration = Duration::from_secs(20);

    fn one_session_at_a_time(test: impl Future<Output = ()>) {
        let _turn = SESSIONS.lock().unwrap_or_else(PoisonError::into_inner);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(test);
        drop(runtime);
    }

    async fn db_host(c: &DIContainer, mac: &str, ip: Option<&str>) -> i64 {
        c.host_repo
            .upsert_host(mac.to_string(), Ok(1_000_000), ip.map(Into::into))
            .await
            .unwrap()
            .id
    }

    async fn image(c: &DIContainer, parttable: &[u8], partition: &[u8]) -> i64 {
        let image = c
            .image_repo
            .create_image(
                format!("cast-{}", IMAGE_ID.fetch_add(1, Ordering::Relaxed)),
                ImageKind::Disk,
            )
            .await
            .unwrap();
        c.image_repo
            .save_partition(image.id, 1, "ext4", partition.len() as i64, None, None)
            .await
            .unwrap();
        let table = PathBuf::from(c.image_service.get_partition_table_path(image.id));
        std::fs::create_dir_all(table.parent().unwrap()).unwrap();
        std::fs::write(&table, parttable).unwrap();
        std::fs::write(c.image_service.get_partition_path(image.id, 1), partition).unwrap();
        c.image_repo.mark_finished(image.id).await.unwrap();
        image.id
    }

    fn receiver(task_id: i64, slot: i64) -> Receiver {
        Receiver::builder()
            .socket(
                Ipv4Addr::LOCALHOST,
                multicast_group(task_id),
                get_multicast_port(slot),
            )
            .expect("bind receiver")
            .max_wait(SETTLE)
            .build()
    }

    fn receive(task_id: i64, slot: i64) -> JoinHandle<Vec<u8>> {
        let receiver = receiver(task_id, slot);
        tokio::spawn(async move {
            let mut received = Vec::new();
            receiver.recv_to(&mut received).await.expect("receive");
            received
        })
    }

    async fn state_of(c: &DIContainer, task_id: i64, host_id: i64) -> TaskState {
        c.task_repo
            .get(task_id)
            .await
            .unwrap()
            .hosts
            .iter()
            .find(|h| h.host_id == host_id)
            .unwrap()
            .state
    }

    async fn eventually(c: &DIContainer, task_id: i64, host_id: i64, state: TaskState) {
        let deadline = Instant::now() + SETTLE;
        while state_of(c, task_id, host_id).await != state {
            assert!(
                Instant::now() < deadline,
                "host {host_id} never reached {state}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[test]
    fn a_session_delivers_every_file_and_fails_only_hosts_that_disconnected() {
        one_session_at_a_time(async {
            let (c, _dir) = container().await;
            let connected = db_host(&c, "aa:bb:cc:dd:ee:01", None).await;
            let gone = db_host(&c, "aa:bb:cc:dd:ee:02", None).await;
            let mut registration = c.host_registry.register(connected);
            let parttable: Vec<u8> = (0..4096).map(|i| (i % 7) as u8).collect();
            let partition: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
            let image_id = image(&c, &parttable, &partition).await;
            let task = c
                .task_repo
                .create(TaskType::Multicast, vec![connected, gone], Some(image_id))
                .await
                .unwrap();
            let receivers = [
                receive(task.id, 0),
                receive(task.id, 0),
                receive(task.id, 1),
                receive(task.id, 1),
            ];

            c.multicast_manager.wake();

            match tokio::time::timeout(SETTLE, registration.receiver.recv()).await {
                Ok(Some(())) => {}
                other => panic!("expected a dispatch at session start, got {other:?}"),
            }
            let next = c.task_repo.get_next(connected).await.unwrap().unwrap();
            assert_eq!(next.id, task.id);
            assert!(next.is_dispatchable_to(connected));
            let [table_a, table_b, data_a, data_b] = receivers;
            assert_eq!(table_a.await.unwrap(), parttable);
            assert_eq!(table_b.await.unwrap(), parttable);
            assert_eq!(data_a.await.unwrap(), partition);
            assert_eq!(data_b.await.unwrap(), partition);

            eventually(&c, task.id, gone, TaskState::Failed).await;
            assert_eq!(state_of(&c, task.id, connected).await, TaskState::Running);
        });
    }

    #[test]
    fn cancelling_a_gathering_session_moves_on_to_the_next_task_in_queue_order() {
        one_session_at_a_time(async {
            let (c, _dir) = container().await;
            let first_host = db_host(&c, "aa:bb:cc:dd:ee:03", None).await;
            let second_host = db_host(&c, "aa:bb:cc:dd:ee:04", None).await;
            let image_id = image(&c, b"table", b"data").await;
            let first = c
                .task_repo
                .create(TaskType::Multicast, vec![first_host], Some(image_id))
                .await
                .unwrap();
            let second = c
                .task_repo
                .create(TaskType::Multicast, vec![second_host], Some(image_id))
                .await
                .unwrap();

            c.multicast_manager.wake();
            eventually(&c, first.id, first_host, TaskState::Running).await;
            assert_eq!(
                state_of(&c, second.id, second_host).await,
                TaskState::Pending
            );

            c.task_repo.cancel(first.id).await.unwrap();
            c.multicast_manager.wake();

            eventually(&c, second.id, second_host, TaskState::Running).await;
            assert_eq!(receive(second.id, 0).await.unwrap(), b"table");
            assert_eq!(
                state_of(&c, first.id, first_host).await,
                TaskState::Cancelled
            );
        });
    }

    #[test]
    fn a_host_busy_with_another_task_is_failed_instead_of_left_running() {
        one_session_at_a_time(async {
            let (c, _dir) = container().await;
            let busy = db_host(&c, "aa:bb:cc:dd:ee:07", None).await;
            let deploy = c
                .task_repo
                .create(TaskType::Deploy, vec![busy], None)
                .await
                .unwrap();
            c.task_repo.start(deploy.id, busy).await.unwrap();
            let image_id = image(&c, b"table", b"data").await;
            let cast = c
                .task_repo
                .create(TaskType::Multicast, vec![busy], Some(image_id))
                .await
                .unwrap();

            c.multicast_manager.wake();

            eventually(&c, cast.id, busy, TaskState::Failed).await;
            assert_eq!(state_of(&c, deploy.id, busy).await, TaskState::Running);
        });
    }

    #[test]
    fn kicking_a_host_evicts_its_receiver_and_reports_it_as_not_joined() {
        one_session_at_a_time(async {
            let (c, _dir) = container().await;
            let kicked = db_host(&c, "aa:bb:cc:dd:ee:08", Some("127.0.0.1")).await;
            let missing = db_host(&c, "aa:bb:cc:dd:ee:09", None).await;
            let image_id = image(&c, b"table", b"data").await;
            let task = c
                .task_repo
                .create(TaskType::Multicast, vec![kicked, missing], Some(image_id))
                .await
                .unwrap();
            let receiving = tokio::spawn(receiver(task.id, 0).recv_to(Vec::new()));
            let mut progress = c.progress_board.watch();

            c.multicast_manager.wake();
            let joined = |snapshot: &Snapshot| {
                snapshot.tasks().iter().any(|t| {
                    t.multicast.as_ref().is_some_and(|m| {
                        m.receivers.iter().any(|r| r.host_id == kicked && r.joined)
                    })
                })
            };
            tokio::time::timeout(SETTLE, progress.wait_for(joined))
                .await
                .unwrap()
                .unwrap();

            c.task_repo
                .mark_failed(task.id, kicked, "kicked")
                .await
                .unwrap();
            c.multicast_manager.wake();

            let outcome = tokio::time::timeout(SETTLE, receiving)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(outcome, Err(ProtoError::Evicted(_))),
                "{outcome:?}"
            );
            tokio::time::timeout(SETTLE, progress.wait_for(|p| !joined(p)))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(state_of(&c, task.id, missing).await, TaskState::Running);
        });
    }

    #[test]
    fn startup_fails_hosts_that_were_mid_transfer_but_keeps_queued_ones() {
        one_session_at_a_time(async {
            let (c, _dir) = container().await;
            let interrupted = db_host(&c, "aa:bb:cc:dd:ee:05", None).await;
            let queued = db_host(&c, "aa:bb:cc:dd:ee:06", None).await;
            let image_id = image(&c, b"table", b"data").await;
            let task = c
                .task_repo
                .create(
                    TaskType::Multicast,
                    vec![interrupted, queued],
                    Some(image_id),
                )
                .await
                .unwrap();
            c.task_repo.start(task.id, interrupted).await.unwrap();

            let _restarted = MulticastManager::start(
                c.task_repo.clone(),
                c.host_repo.clone(),
                c.image_repo.clone(),
                c.image_service.clone(),
                c.host_registry.clone(),
                c.progress_board.clone(),
                "lo".to_string(),
            )
            .await
            .unwrap();

            let rows = c.task_repo.get(task.id).await.unwrap();
            let row = |id: i64| rows.hosts.iter().find(|h| h.host_id == id).unwrap();
            assert_eq!(row(interrupted).state, TaskState::Failed);
            assert_eq!(row(interrupted).error.as_deref(), Some(ORPHANED));
            assert_ne!(row(queued).state, TaskState::Failed);
        });
    }
}
