use crate::{
    domain::task::{Task, TaskState, TaskType},
    error::{AppError, Result},
};
use imaged_shared::{MULTICAST_GROUP_ADDRESS, get_multicast_port};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tracing::{debug, error, info};

use scuttlecast::{
    sender::Sender,
    state::{LimitingFactor, ReceiverState, TransferState},
};
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

use crate::{
    domain::{host::HostRepository, image::ImageRepository, task::TaskRepository},
    service::image::ImageService,
};

struct RunningMulticastTask {
    id: i64,
    _handle: JoinHandle<()>,
    /// Cancels the in-flight `do_work` for `id`, dropping the running send.
    cancel: CancellationToken,
    /// Set by `notify_new` under the slot lock when work arrives while this
    /// worker is alive. The worker's exit path re-reads it under the same lock
    /// and keeps looping instead of exiting, so a task enqueued during the
    /// exit window is never dropped.
    pending: bool,
    generation: u64,
}

struct SlotGuard {
    slot: Arc<Mutex<Option<RunningMulticastTask>>>,
    generation: u64,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut slot = match self.slot.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.as_ref().map(|r| r.generation) == Some(self.generation) {
            *slot = None;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MulticastProgress {
    pub task_id: i64,
    pub fraction: f64,
    pub bytes_per_second: f64,
    pub receivers: usize,
    pub step: usize,
    pub steps: usize,
    pub eta: Option<Duration>,
}

#[derive(Clone, Copy)]
struct TransferStep {
    task_id: i64,
    step: usize,
    steps: usize,
    bytes_before: u64,
    total_bytes: u64,
}

impl TransferStep {
    fn progress(&self, state: &TransferState) -> MulticastProgress {
        let sent = self.bytes_before + state.bytes_sent();
        let left = self.total_bytes.saturating_sub(sent);
        let bytes_per_second = state.bytes_per_second();
        MulticastProgress {
            task_id: self.task_id,
            fraction: match self.total_bytes {
                0 => 0.0,
                total => (sent as f64 / total as f64).min(1.0),
            },
            bytes_per_second,
            receivers: state.receivers.len(),
            step: self.step,
            steps: self.steps,
            eta: (bytes_per_second > 0.0)
                .then(|| Duration::try_from_secs_f64(left as f64 / bytes_per_second).ok())
                .flatten(),
        }
    }
}

#[derive(Clone)]
pub struct MulticastManager {
    host_repo: Arc<dyn HostRepository>,
    task_repo: Arc<dyn TaskRepository>,
    image_repo: Arc<dyn ImageRepository>,
    image_service: Arc<ImageService>,
    interface: String,
    current: Arc<Mutex<Option<RunningMulticastTask>>>,
    next_generation: Arc<AtomicU64>,
    progress: Arc<watch::Sender<Option<MulticastProgress>>>,
}

impl MulticastManager {
    pub async fn new(
        host_repo: Arc<dyn HostRepository>,
        task_repo: Arc<dyn TaskRepository>,
        image_repo: Arc<dyn ImageRepository>,
        image_service: Arc<ImageService>,
        interface: String,
    ) -> Result<Self> {
        let new = Self {
            host_repo,
            task_repo: task_repo.clone(),
            image_repo,
            image_service,
            interface,
            current: Arc::new(Mutex::new(None)),
            next_generation: Arc::new(AtomicU64::new(0)),
            progress: Arc::new(watch::Sender::new(None)),
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

        if let Some(r) = lock.as_mut() {
            r.pending = true;
            return Ok(());
        }

        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let self_clone = self.clone();
        let handle = tokio::spawn(async move {
            let _guard = SlotGuard {
                slot: self_clone.current.clone(),
                generation,
            };
            if let Err(e) = self_clone.handle_loop().await {
                error!(err=%e, "multicast task handler failed");
            }
        });

        *lock = Some(RunningMulticastTask {
            _handle: handle,
            // this id can be incorrect since the loop fetches the task for himself
            id: task_id,
            cancel: CancellationToken::new(),
            pending: false,
            generation,
        });

        Ok(())
    }

    /// Cancel the in-flight multicast send if `task_id` is the one currently
    /// running. The caller is responsible for marking the task cancelled in the
    /// DB first; this stops the detached sender loop's current `do_work` so the
    /// loop moves on to the next queued task.
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
        loop {
            let Some(t) = self.task_repo.get_next_multicast().await? else {
                // Empty queue: decide whether to exit under the same lock
                // notify_new uses to publish work. A task enqueued while this
                // worker was polling sets `pending`, which we observe here and
                // keep looping instead of exiting.
                let mut lock = self.current.lock().unwrap();
                if let Some(r) = lock.as_mut()
                    && r.pending
                {
                    r.pending = false;
                    continue;
                }
                *lock = None;
                return Ok(());
            };
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
                // Cancel wins: dropping the do_work future drops the sender and
                // its socket. The DB rows are already marked cancelled by the
                // caller, so nothing to do here.
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
            self.progress.send_replace(None);
        }
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
        let targets = Arc::new(self.target_names(&task).await);
        let partitions = self.image_repo.get_partitions(image_id).await?;

        let mut plan = vec![(
            self.image_service.get_partition_table_path(image_id),
            get_multicast_port(0),
            0u64,
        )];
        for p in &partitions {
            plan.push((
                self.image_service
                    .get_partition_path(image_id, p.partition_number),
                get_multicast_port(p.partition_number),
                0,
            ));
        }
        for entry in plan.iter_mut() {
            entry.2 = tokio::fs::metadata(&entry.0)
                .await
                .map(|m| m.len())
                .unwrap_or_default();
        }

        let total_bytes = plan.iter().map(|(_, _, size)| size).sum();
        let steps = plan.len();
        let mut bytes_before = 0;
        for (step, (path, port, size)) in plan.into_iter().enumerate() {
            info!(task_id=%task.id, image_id=%image_id, step=%step, steps=%steps, "sending file over multicast");
            self.send_file(
                &path,
                port,
                num_receivers,
                TransferStep {
                    task_id: task.id,
                    step: step + 1,
                    steps,
                    bytes_before,
                    total_bytes,
                },
                Arc::clone(&targets),
            )
            .await?;
            bytes_before += size;
        }

        Ok(())
    }

    pub fn progress(&self) -> watch::Receiver<Option<MulticastProgress>> {
        self.progress.subscribe()
    }

    async fn target_names(&self, task: &Task) -> HashMap<IpAddr, String> {
        let hosts = match self.host_repo.get_all(None).await {
            Ok(hosts) => hosts,
            Err(e) => {
                error!(err=%e, "could not resolve multicast receivers to hosts");
                return HashMap::new();
            }
        };
        hosts
            .into_iter()
            .filter(|host| task.hosts.iter().any(|h| h.host_id == host.id))
            .filter_map(|host| {
                let ip = host.ip.as_ref()?.parse().ok()?;
                Some((ip, format!("{} (#{})", host.name, host.id)))
            })
            .collect()
    }

    async fn send_file(
        &self,
        file: &str,
        port: u16,
        num_receivers: usize,
        step: TransferStep,
        targets: Arc<HashMap<IpAddr, String>>,
    ) -> Result {
        let sender = Sender::builder()
            .socket(
                interface_address(&self.interface)?,
                MULTICAST_GROUP_ADDRESS,
                port,
            )
            .map_err(|e| AppError::Internal(format!("failed to bind multicast sender: {e}")))?
            .min_receivers(num_receivers)
            .build();

        let _reporter = AbortOnDropHandle::new(tokio::spawn(report_progress(
            sender.progress(),
            Arc::clone(&self.progress),
            step,
            targets,
        )));

        sender
            .send_file(PathBuf::from(file))
            .await
            .map_err(|e| AppError::Internal(format!("multicast send of {file} failed: {e}")))
    }
}

fn interface_address(name: &str) -> Result<Ipv4Addr> {
    local_ip_address::list_afinet_netifas()
        .map_err(|e| AppError::Internal(format!("failed to list network interfaces: {e}")))?
        .into_iter()
        .find_map(|(iface, address)| match address {
            IpAddr::V4(v4) if iface == name => Some(v4),
            _ => None,
        })
        .ok_or_else(|| AppError::Internal(format!("interface {name} has no IPv4 address")))
}

const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);
const PROGRESS_PUBLISH_INTERVAL: Duration = Duration::from_millis(250);

fn limiting_receiver(state: &TransferState) -> Option<&ReceiverState> {
    let id = match state.limiting {
        LimitingFactor::WindowStalled { blocked_by, .. } => blocked_by,
        LimitingFactor::RateLimited { worst, .. } => worst,
        LimitingFactor::SinkStalled { receiver, .. } => receiver,
        _ => return None,
    };
    state.receivers.iter().find(|r| r.receiver_id == id)
}

async fn report_progress(
    transfer: watch::Receiver<TransferState>,
    publisher: Arc<watch::Sender<Option<MulticastProgress>>>,
    step: TransferStep,
    targets: Arc<HashMap<IpAddr, String>>,
) {
    let mut publish = tokio::time::interval(PROGRESS_PUBLISH_INTERVAL);
    let mut log = tokio::time::interval(PROGRESS_LOG_INTERVAL);
    log.tick().await;
    loop {
        tokio::select! {
            _ = publish.tick() => {
                publisher.send_replace(Some(step.progress(&transfer.borrow())));
            }
            _ = log.tick() => log_progress(&transfer.borrow(), step.task_id, &targets),
        }
    }
}

fn receiver_name(targets: &HashMap<IpAddr, String>, receiver: &ReceiverState) -> String {
    targets
        .get(&receiver.address.ip())
        .cloned()
        .unwrap_or_else(|| receiver.address.ip().to_string())
}

fn log_progress(state: &TransferState, task_id: i64, targets: &HashMap<IpAddr, String>) {
    let culprit = limiting_receiver(state);
    info!(
        task_id,
        mbit_per_second = state.bytes_per_second() * 8.0 / 1e6,
        percent_complete = ?state.fraction_complete().map(|f| f * 100.0),
        blocks_sent = state.blocks_sent,
        receivers = state.receivers.len(),
        parity_shards = state.parity_shards,
        draining = state.draining,
        limiting = %state.limiting,
        limiting_host = ?culprit.map(|r| receiver_name(targets, r)),
        limiting_host_loss = ?culprit.map(|r| r.unrecovered_loss),
        limiting_host_stall_ms = ?culprit.map(|r| r.sink_stall_ms),
        slowest_host = ?state.slowest().map(|r| (receiver_name(targets, r), r.slices_behind)),
        "multicast transfer progress"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use scuttlecast::receiver::Receiver;
    use std::net::Ipv4Addr;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    fn receiver_state(receiver_id: u64, address: &str) -> ReceiverState {
        ReceiverState {
            receiver_id,
            address: address.parse().unwrap(),
            windowed_loss: 0.0,
            unrecovered_loss: 0.0,
            lifetime_loss: 0.0,
            next_needed_slice: 0,
            slices_behind: 0,
            naks: 0,
            sink_stall_ms: 0,
        }
    }

    #[test]
    fn the_limiting_factor_is_resolved_to_the_host_holding_the_deploy_up() {
        let state = TransferState {
            receivers: vec![
                receiver_state(11, "10.0.0.1:5000"),
                receiver_state(22, "10.0.0.2:5000"),
            ],
            limiting: LimitingFactor::SinkStalled {
                receiver: 22,
                stall_ms: 80,
            },
            ..TransferState::default()
        };

        assert_eq!(
            limiting_receiver(&state).map(|r| r.address.to_string()),
            Some("10.0.0.2:5000".to_string())
        );
    }

    #[test]
    fn a_healthy_transfer_blames_no_host() {
        let state = TransferState {
            receivers: vec![receiver_state(11, "10.0.0.1:5000")],
            limiting: LimitingFactor::Unconstrained,
            ..TransferState::default()
        };

        assert!(limiting_receiver(&state).is_none());
    }

    #[test]
    fn a_server_side_bottleneck_blames_no_host() {
        let state = TransferState {
            receivers: vec![receiver_state(11, "10.0.0.1:5000")],
            limiting: LimitingFactor::SourceStarved { read_wait_ms: 40 },
            ..TransferState::default()
        };

        assert!(limiting_receiver(&state).is_none());
    }

    fn sending_state(blocks_sent: u64, blocks_per_second: f64) -> TransferState {
        TransferState {
            block_size: 100,
            blocks_sent,
            blocks_per_second,
            receivers: vec![receiver_state(11, "10.0.0.1:5000")],
            ..TransferState::default()
        }
    }

    #[tokio::test]
    async fn the_progress_log_names_receivers_by_host_instead_of_a_bare_address() {
        let manager = build_manager_with_hosts(
            Arc::new(FakeTaskRepo::new()),
            Arc::new(FakeImageRepo),
            vec![
                Host::new(7, "lab-07".into(), "aa:bb".into(), 0, Some("10.0.0.1".into())),
                Host::new(9, "lab-09".into(), "cc:dd".into(), 0, Some("10.0.0.9".into())),
            ],
        );
        let mut task = mk_task(1, TaskType::Multicast, TaskState::Pending, Some(1));
        task.hosts[0].host_id = 7;

        let targets = manager.target_names(&task).await;

        assert_eq!(
            receiver_name(&targets, &receiver_state(11, "10.0.0.1:5000")),
            "lab-07 (#7)"
        );
        assert_eq!(
            receiver_name(&targets, &receiver_state(22, "10.0.0.9:5000")),
            "10.0.0.9",
            "a host that is not part of the task keeps its raw address"
        );
        assert_eq!(
            receiver_name(&targets, &receiver_state(33, "10.0.0.5:5000")),
            "10.0.0.5"
        );
    }

    fn step(bytes_before: u64, total_bytes: u64) -> TransferStep {
        TransferStep {
            task_id: 4,
            step: 2,
            steps: 3,
            bytes_before,
            total_bytes,
        }
    }

    #[test]
    fn progress_covers_the_whole_task_rather_than_the_file_being_sent() {
        let reported = step(1000, 4000).progress(&sending_state(10, 5.0));

        assert_eq!(reported.task_id, 4);
        assert_eq!(reported.fraction, 0.5);
        assert_eq!(reported.bytes_per_second, 500.0);
        assert_eq!(reported.eta, Some(Duration::from_secs(4)));
        assert_eq!((reported.step, reported.steps), (2, 3));
        assert_eq!(reported.receivers, 1);
    }

    #[test]
    fn a_stalled_transfer_reports_progress_without_an_eta() {
        let reported = step(1000, 4000).progress(&sending_state(0, 0.0));

        assert_eq!(reported.fraction, 0.25);
        assert_eq!(reported.eta, None);
    }

    #[test]
    fn progress_stays_in_range_when_sizes_are_unknown_or_overshot() {
        assert_eq!(step(0, 0).progress(&sending_state(10, 1.0)).fraction, 0.0);
        assert_eq!(step(0, 500).progress(&sending_state(10, 1.0)).fraction, 1.0);
    }

    const CANCEL_TEST_PORT: u16 = 50_900;
    const CANCEL_TEST_BLOCKS_PER_SECOND: f64 = 200.0;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelling_stops_the_transfer_on_the_wire() {
        let payload = vec![0xA5u8; 4 * 1024 * 1024];
        let payload_len = payload.len() as u64;

        let receiver = Receiver::builder()
            .socket(
                Ipv4Addr::LOCALHOST,
                MULTICAST_GROUP_ADDRESS,
                CANCEL_TEST_PORT,
            )
            .expect("bind receiver")
            .max_wait(Duration::from_secs(30))
            .build();

        let received = Arc::new(AtomicU64::new(0));
        let (pipe, mut drain) = tokio::io::duplex(1024 * 1024);
        let counter = received.clone();
        let draining = tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = drain.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                counter.fetch_add(n as u64, Ordering::SeqCst);
            }
        });
        let receiving = tokio::spawn(receiver.recv_to(pipe));

        let token = CancellationToken::new();
        let sending = tokio::spawn({
            let token = token.clone();
            async move {
                let sender = Sender::builder()
                    .socket(
                        Ipv4Addr::LOCALHOST,
                        MULTICAST_GROUP_ADDRESS,
                        CANCEL_TEST_PORT,
                    )
                    .expect("bind sender")
                    .min_receivers(1)
                    .max_rate(CANCEL_TEST_BLOCKS_PER_SECOND)
                    .build();
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {}
                    res = sender.send_stream(std::io::Cursor::new(payload), Some(payload_len)) => {
                        panic!("send completed instead of being cancelled: {res:?}")
                    }
                }
            }
        });

        while received.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        token.cancel();
        sending.await.expect("send task");

        tokio::time::sleep(Duration::from_millis(500)).await;
        let settled = received.load(Ordering::SeqCst);
        assert!(
            settled < payload_len,
            "the whole payload arrived before cancellation could take effect, so this test proves nothing; lower CANCEL_TEST_BLOCKS_PER_SECOND"
        );

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            received.load(Ordering::SeqCst),
            settled,
            "bytes kept arriving after the send was cancelled"
        );

        receiving.abort();
        draining.abort();
    }

    #[test]
    fn the_loopback_interface_resolves_to_its_ipv4_address() {
        assert_eq!(interface_address("lo").unwrap(), Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn an_unknown_interface_is_an_error_rather_than_a_silent_default() {
        assert!(interface_address("definitely-not-an-interface").is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn send_file_delivers_the_whole_file_to_a_receiver_on_that_interface() {
        const PORT: u16 = 50_904;
        let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();

        let path = std::env::temp_dir().join(format!("imaged-send-file-{}", std::process::id()));
        std::fs::write(&path, &payload).expect("write payload");

        let receiver = Receiver::builder()
            .socket(Ipv4Addr::LOCALHOST, MULTICAST_GROUP_ADDRESS, PORT)
            .expect("bind receiver")
            .max_wait(Duration::from_secs(30))
            .build();
        let receiving = tokio::spawn(async move {
            let mut received = Vec::new();
            let summary = receiver.recv_to(&mut received).await.expect("receive");
            (received, summary)
        });

        let manager = build_manager(Arc::new(FakeTaskRepo::new()), Arc::new(FakeImageRepo));
        manager
            .send_file(
                path.to_str().unwrap_or_default(),
                PORT,
                1,
                TransferStep {
                    task_id: 1,
                    step: 1,
                    steps: 1,
                    bytes_before: 0,
                    total_bytes: payload.len() as u64,
                },
                Arc::new(HashMap::new()),
            )
            .await
            .expect("send");

        assert_eq!(
            manager.progress().borrow().as_ref().map(|p| p.task_id),
            Some(1)
        );

        let (received, summary) = receiving.await.expect("receive task");
        let _ = std::fs::remove_file(&path);

        assert_eq!(received, payload);
        assert_eq!(summary.total_bytes, payload.len() as u64);
    }

    use crate::di::DIContainer;
    use crate::domain::host::Host;
    use crate::domain::image::{Image, ImagePartition, ImageStatus};
    use crate::domain::task::TaskHost;
    use chrono::Utc;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    fn mk_task(id: i64, task_type: TaskType, state: TaskState, image_id: Option<i64>) -> Task {
        Task {
            id,
            task_type,
            hosts: vec![TaskHost {
                host_id: 1,
                state,
                error: None,
                started_at: None,
                finished_at: None,
            }],
            image_id,
            image_name: None,
            image_deleted: false,
            created_at: Utc::now(),
        }
    }

    struct FakeTaskRepo {
        queue: Mutex<VecDeque<Task>>,
        get_next_calls: AtomicUsize,
        get_ids: Mutex<Vec<i64>>,
        start_ids: Mutex<Vec<(i64, i64)>>,
        failed_ids: Mutex<Vec<i64>>,
        finished_ids: Mutex<Vec<i64>>,
        gate: AtomicBool,
        panic_on_get_next: AtomicBool,
        park_on_empty: AtomicBool,
        parked_at_empty: AtomicBool,
        release_park: AtomicBool,
        get_template: Mutex<Task>,
    }

    impl FakeTaskRepo {
        fn new() -> Self {
            Self {
                queue: Mutex::new(VecDeque::new()),
                get_next_calls: AtomicUsize::new(0),
                get_ids: Mutex::new(Vec::new()),
                start_ids: Mutex::new(Vec::new()),
                failed_ids: Mutex::new(Vec::new()),
                finished_ids: Mutex::new(Vec::new()),
                gate: AtomicBool::new(false),
                panic_on_get_next: AtomicBool::new(false),
                park_on_empty: AtomicBool::new(false),
                parked_at_empty: AtomicBool::new(false),
                release_park: AtomicBool::new(false),
                get_template: Mutex::new(mk_task(0, TaskType::Multicast, TaskState::Pending, None)),
            }
        }
    }

    #[async_trait::async_trait]
    impl TaskRepository for FakeTaskRepo {
        async fn create(
            &self,
            _task_type: TaskType,
            _host_ids: Vec<i64>,
            _image_id: Option<i64>,
        ) -> Result<Task> {
            unimplemented!()
        }
        async fn get_next(&self, _host_id: i64) -> Result<Option<Task>> {
            unimplemented!()
        }
        async fn start(&self, task_id: i64, host_id: i64) -> Result {
            self.start_ids.lock().unwrap().push((task_id, host_id));
            Ok(())
        }
        async fn mark_finished(&self, _task_id: i64, _host_id: i64) -> Result {
            unimplemented!()
        }
        async fn mark_failed(&self, _task_id: i64, _host_id: i64, _error: &str) -> Result {
            unimplemented!()
        }
        async fn mark_all_finished(&self, task_id: i64) -> Result {
            self.finished_ids.lock().unwrap().push(task_id);
            Ok(())
        }
        async fn mark_all_failed(&self, task_id: i64, _error: &str) -> Result {
            self.failed_ids.lock().unwrap().push(task_id);
            Ok(())
        }
        async fn retry(&self, _id: i64) -> Result {
            unimplemented!()
        }
        async fn cancel(&self, _id: i64) -> Result {
            unimplemented!()
        }
        async fn get_active_by_image(&self, _image_id: i64) -> Result<Vec<Task>> {
            unimplemented!()
        }
        async fn get_all(&self) -> Result<Vec<Task>> {
            unimplemented!()
        }
        async fn get(&self, id: i64) -> Result<Task> {
            self.get_ids.lock().unwrap().push(id);
            let mut t = self.get_template.lock().unwrap().clone();
            t.id = id;
            Ok(t)
        }
        async fn get_next_multicast(&self) -> Result<Option<Task>> {
            self.get_next_calls.fetch_add(1, Ordering::SeqCst);
            if self.panic_on_get_next.load(Ordering::SeqCst) {
                panic!("injected panic in get_next_multicast");
            }
            while self.gate.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            let popped = self.queue.lock().unwrap().pop_front();
            if popped.is_none() && self.park_on_empty.load(Ordering::SeqCst) {
                self.parked_at_empty.store(true, Ordering::SeqCst);
                while !self.release_park.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
                return Ok(None);
            }
            Ok(popped)
        }
    }

    struct FakeImageRepo;

    #[async_trait::async_trait]
    impl ImageRepository for FakeImageRepo {
        async fn get_status(&self, _id: i64) -> Result<ImageStatus> {
            unimplemented!()
        }
        async fn create_image(&self, _name: String) -> Result<Image> {
            unimplemented!()
        }
        async fn update_name(&self, _id: i64, _new_name: String) -> Result<Image> {
            unimplemented!()
        }
        async fn get_all(&self) -> Result<Vec<Image>> {
            unimplemented!()
        }
        async fn save_partition(
            &self,
            _image_id: i64,
            _partition_number: i64,
            _fstype: &str,
            _size_bytes: i64,
        ) -> Result<ImagePartition> {
            unimplemented!()
        }
        async fn delete_image(&self, _id: i64) -> Result {
            unimplemented!()
        }
        async fn start_capture(&self, _id: i64) -> Result {
            unimplemented!()
        }
        async fn mark_finished(&self, _id: i64) -> Result {
            unimplemented!()
        }
        async fn mark_faulted(&self, _id: i64, _error: &str) -> Result {
            unimplemented!()
        }
        async fn get_partitions(&self, _id: i64) -> Result<Vec<ImagePartition>> {
            unimplemented!()
        }
    }

    struct FakeHostRepo(Vec<Host>);

    impl FakeHostRepo {
        fn unused() -> AppError {
            AppError::Internal("unused by this test".to_string())
        }
    }

    #[async_trait::async_trait]
    impl HostRepository for FakeHostRepo {
        async fn upsert_host(&self, _: String, _: u64, _: Option<String>) -> Result<Host> {
            Err(Self::unused())
        }
        async fn update_name(&self, _: i64, _: String) -> Result<Host> {
            Err(Self::unused())
        }
        async fn get_all(&self, _: Option<i64>) -> Result<Vec<Host>> {
            Ok(self
                .0
                .iter()
                .map(|h| Host::new(h.id, h.name.clone(), h.mac_address.clone(), h.disk_size, h.ip.clone()))
                .collect())
        }
        async fn delete(&self, _: i64) -> Result {
            Err(Self::unused())
        }
        async fn get_by_mac(&self, _: &str) -> Result<Host> {
            Err(Self::unused())
        }
    }

    fn build_manager(
        task_repo: Arc<FakeTaskRepo>,
        image_repo: Arc<FakeImageRepo>,
    ) -> MulticastManager {
        build_manager_with_hosts(task_repo, image_repo, Vec::new())
    }

    fn build_manager_with_hosts(
        task_repo: Arc<FakeTaskRepo>,
        image_repo: Arc<FakeImageRepo>,
        hosts: Vec<Host>,
    ) -> MulticastManager {
        MulticastManager {
            host_repo: Arc::new(FakeHostRepo(hosts)),
            task_repo: task_repo as Arc<dyn TaskRepository>,
            image_repo: image_repo as Arc<dyn ImageRepository>,
            image_service: Arc::new(ImageService::new("/tmp/imaged-mcmgr-unused".to_string())),
            interface: "lo".to_string(),
            current: Arc::new(Mutex::new(None)),
            next_generation: Arc::new(AtomicU64::new(0)),
            progress: Arc::new(watch::Sender::new(None)),
        }
    }

    async fn wait_for<F: FnMut() -> bool>(mut cond: F) -> bool {
        for _ in 0..100_000 {
            if cond() {
                return true;
            }
            tokio::task::yield_now().await;
        }
        cond()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_new_does_not_spawn_a_second_worker_while_one_is_live() {
        let repo = Arc::new(FakeTaskRepo::new());
        repo.gate.store(true, Ordering::SeqCst);
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        manager.notify_new(1).unwrap();
        assert!(wait_for(|| repo.get_next_calls.load(Ordering::SeqCst) >= 1).await);

        manager.notify_new(2).unwrap();
        for _ in 0..2000 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            repo.get_next_calls.load(Ordering::SeqCst),
            1,
            "notify_new spawned a second worker while one was already live"
        );
        assert!(manager.current.lock().unwrap().is_some());

        repo.gate.store(false, Ordering::SeqCst);
        assert!(wait_for(|| manager.current.lock().unwrap().is_none()).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_new_spawns_a_fresh_worker_after_the_previous_one_finished() {
        let repo = Arc::new(FakeTaskRepo::new());
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        manager.notify_new(1).unwrap();
        assert!(
            wait_for(|| repo.get_next_calls.load(Ordering::SeqCst) >= 1
                && manager.current.lock().unwrap().is_none())
            .await
        );

        manager.notify_new(2).unwrap();
        assert!(wait_for(|| repo.get_next_calls.load(Ordering::SeqCst) >= 2).await);
        assert_eq!(repo.get_next_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_only_cancels_the_currently_running_task_id() {
        let manager = build_manager(Arc::new(FakeTaskRepo::new()), Arc::new(FakeImageRepo));
        let token = CancellationToken::new();
        let handle = tokio::spawn(std::future::pending::<()>());
        *manager.current.lock().unwrap() = Some(RunningMulticastTask {
            id: 42,
            _handle: handle,
            cancel: token.clone(),
            pending: false,
            generation: 0,
        });

        manager.cancel(99);
        assert!(
            !token.is_cancelled(),
            "cancel(other_id) wrongly cancelled the running task"
        );

        manager.cancel(42);
        assert!(
            token.is_cancelled(),
            "cancel(current_id) did not cancel the running task"
        );

        if let Some(r) = manager.current.lock().unwrap().as_ref() {
            r._handle.abort();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slot_guard_clears_slot_when_worker_returns_early() {
        let repo = Arc::new(FakeTaskRepo::new());
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        manager.notify_new(1).unwrap();
        assert!(
            wait_for(|| manager.current.lock().unwrap().is_none()).await,
            "slot was not cleared after the worker returned"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slot_guard_clears_slot_when_worker_panics() {
        let repo = Arc::new(FakeTaskRepo::new());
        repo.panic_on_get_next.store(true, Ordering::SeqCst);
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        manager.notify_new(1).unwrap();
        assert!(
            wait_for(|| repo.get_next_calls.load(Ordering::SeqCst) >= 1
                && manager.current.lock().unwrap().is_none())
            .await,
            "slot was not cleared after the worker panicked; the manager is wedged"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_loop_drains_tasks_in_get_next_multicast_order_and_exits_when_empty() {
        let repo = Arc::new(FakeTaskRepo::new());
        {
            let mut q = repo.queue.lock().unwrap();
            q.push_back(mk_task(10, TaskType::Multicast, TaskState::Pending, None));
            q.push_back(mk_task(20, TaskType::Multicast, TaskState::Pending, None));
            q.push_back(mk_task(30, TaskType::Multicast, TaskState::Pending, None));
        }
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        manager.notify_new(10).unwrap();
        assert!(wait_for(|| manager.current.lock().unwrap().is_none()).await);

        assert_eq!(
            *repo.get_ids.lock().unwrap(),
            vec![10, 20, 30],
            "do_work was not attempted for each task in get_next_multicast order"
        );
        assert_eq!(
            repo.get_next_calls.load(Ordering::SeqCst),
            4,
            "loop did not poll once more to observe the empty queue and exit"
        );
        assert_eq!(*repo.failed_ids.lock().unwrap(), vec![10, 20, 30]);
        assert!(repo.start_ids.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn do_work_rejects_wrong_task_type_without_spawning_the_sender() {
        let repo = Arc::new(FakeTaskRepo::new());
        *repo.get_template.lock().unwrap() =
            mk_task(0, TaskType::Deploy, TaskState::Pending, Some(1));
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        let err = manager
            .do_work(mk_task(5, TaskType::Multicast, TaskState::Pending, Some(1)))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::FailedPrecondition(_)));
        assert!(
            repo.start_ids.lock().unwrap().is_empty(),
            "do_work entered the send path for a wrong task type"
        );
    }

    #[tokio::test]
    async fn do_work_rejects_non_pending_state_without_spawning_the_sender() {
        let repo = Arc::new(FakeTaskRepo::new());
        *repo.get_template.lock().unwrap() =
            mk_task(0, TaskType::Multicast, TaskState::Done, Some(1));
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        let err = manager
            .do_work(mk_task(5, TaskType::Multicast, TaskState::Pending, Some(1)))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::FailedPrecondition(_)));
        assert!(
            repo.start_ids.lock().unwrap().is_empty(),
            "do_work entered the send path for a non-pending task"
        );
    }

    #[tokio::test]
    async fn do_work_rejects_missing_image_id_without_spawning_the_sender() {
        let repo = Arc::new(FakeTaskRepo::new());
        *repo.get_template.lock().unwrap() =
            mk_task(0, TaskType::Multicast, TaskState::Pending, None);
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        let err = manager
            .do_work(mk_task(5, TaskType::Multicast, TaskState::Pending, None))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::FailedPrecondition(_)));
        assert!(
            repo.start_ids.lock().unwrap().is_empty(),
            "do_work entered the send path for a task with no image"
        );
    }

    static DB_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn container() -> (DIContainer, TestDir) {
        let id = DB_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("imaged-mcmgr-{}-{}", std::process::id(), id));
        let c = crate::build_test_container(&dir).await;
        (c, TestDir(dir))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_marks_orphaned_running_multicast_tasks_failed_and_kicks_off_next_pending() {
        let (c, _dir) = container().await;
        let host = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:ff".to_string(), 100, None)
            .await
            .unwrap();
        let img = c
            .image_repo
            .create_image("recovery-image".to_string())
            .await
            .unwrap();

        let orphan = c
            .task_repo
            .create(TaskType::Multicast, vec![host.id], Some(img.id))
            .await
            .unwrap();
        c.task_repo.start(orphan.id, host.id).await.unwrap();

        let pending = c
            .task_repo
            .create(TaskType::Multicast, vec![host.id], None)
            .await
            .unwrap();

        let _manager = MulticastManager::new(
            c.host_repo.clone(),
            c.task_repo.clone(),
            c.image_repo.clone(),
            c.image_service.clone(),
            "lo".to_string(),
        )
        .await
        .unwrap();

        assert_eq!(
            c.task_repo.get(orphan.id).await.unwrap().aggregate_state(),
            TaskState::Failed,
            "orphaned running multicast task was not marked failed on startup"
        );

        let mut kicked_off = false;
        for _ in 0..100_000 {
            if c.task_repo.get(pending.id).await.unwrap().aggregate_state() == TaskState::Failed {
                kicked_off = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            kicked_off,
            "the next pending multicast task was not picked up by the recovery-spawned worker"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_new_does_not_drop_a_task_enqueued_while_a_worker_is_exiting() {
        let repo = Arc::new(FakeTaskRepo::new());
        repo.park_on_empty.store(true, Ordering::SeqCst);
        let manager = build_manager(repo.clone(), Arc::new(FakeImageRepo));

        manager.notify_new(1).unwrap();
        assert!(wait_for(|| repo.parked_at_empty.load(Ordering::SeqCst)).await);

        repo.queue.lock().unwrap().push_back(mk_task(
            2,
            TaskType::Multicast,
            TaskState::Pending,
            None,
        ));
        manager.notify_new(2).unwrap();

        repo.release_park.store(true, Ordering::SeqCst);
        assert!(wait_for(|| manager.current.lock().unwrap().is_none()).await);

        assert!(
            wait_for(|| repo.get_ids.lock().unwrap().contains(&2)).await,
            "task 2 was dropped on the floor: no worker ever ran do_work for it"
        );
    }
}
