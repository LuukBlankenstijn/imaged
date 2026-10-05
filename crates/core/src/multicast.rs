use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use futures::FutureExt;
use imaged_shared::{MULTICAST_JOIN_WINDOW, get_multicast_port, multicast_group};
use scuttlecast::{
    error::ProtoError,
    sender::{Sender, Sending},
    state::{LimitingFactor, ReceiverState, TransferState},
};
use tokio::{sync::watch, time::Instant};
use tokio_util::task::AbortOnDropHandle;
use tracing::{error, info, warn};

use crate::{
    domain::{
        host::{Host, HostRepository},
        image::ImageRepository,
        task::{Task, TaskRepository, TaskState, TaskType},
    },
    error::{AppError, Result},
    registry::HostRegistry,
    service::image::ImageService,
};

const DB_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);
const PORT_RELEASE_WAIT: Duration = Duration::from_secs(2);
const PORT_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const ORPHANED: &str = "Server stopped while task was running";
const DISCONNECTED: &str = "Host disconnected before reporting the multicast result";

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

pub struct MulticastManager {
    wake: watch::Sender<()>,
    progress: watch::Receiver<Option<MulticastProgress>>,
    _worker: AbortOnDropHandle<()>,
}

impl MulticastManager {
    pub async fn start(
        tasks: Arc<dyn TaskRepository>,
        hosts: Arc<dyn HostRepository>,
        images: Arc<dyn ImageRepository>,
        image_service: Arc<ImageService>,
        registry: Arc<HostRegistry>,
        interface: String,
    ) -> Result<Self> {
        let (published, progress) = watch::channel(None);
        let worker = Worker {
            tasks,
            hosts,
            images,
            image_service,
            registry,
            interface,
            progress: published,
        };
        worker.fail_orphans().await?;
        let (wake, mut woken) = watch::channel(());
        if worker.tasks.get_next_multicast().await?.is_some() {
            woken.mark_changed();
        }
        Ok(Self {
            wake,
            progress,
            _worker: AbortOnDropHandle::new(tokio::spawn(worker.run(woken))),
        })
    }

    pub fn wake(&self) {
        self.wake.send_replace(());
    }

    pub fn progress(&self) -> watch::Receiver<Option<MulticastProgress>> {
        self.progress.clone()
    }
}

struct Worker {
    tasks: Arc<dyn TaskRepository>,
    hosts: Arc<dyn HostRepository>,
    images: Arc<dyn ImageRepository>,
    image_service: Arc<ImageService>,
    registry: Arc<HostRegistry>,
    interface: String,
    progress: watch::Sender<Option<MulticastProgress>>,
}

struct File {
    path: PathBuf,
    port: u16,
    size: u64,
}

impl Worker {
    async fn fail_orphans(&self) -> Result {
        for task in self.tasks.get_all().await? {
            if task.task_type != TaskType::Multicast {
                continue;
            }
            for host in task.hosts_in(TaskState::Running) {
                self.tasks.mark_failed(task.id, host, ORPHANED).await?;
            }
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
        self.progress.send_replace(None);
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
                self.registry.cancel_task(host, task_id);
            } else if !connected.contains(&host) {
                self.tasks.mark_failed(task_id, host, DISCONNECTED).await?;
            }
        }
        Ok(())
    }

    async fn session(&self, task: &Task, woken: &mut watch::Receiver<()>) -> Result {
        let image_id = task.image_id.ok_or_else(|| {
            AppError::FailedPrecondition("multicast task has no image".to_string())
        })?;
        let participants = self.running(task.id).await?;
        for &host in &participants {
            self.registry.send_task(host, task);
        }

        let files = self.files(image_id).await?;
        let targets = Targets::new(self.hosts.get_all(None).await?, &participants);
        let local_ip = interface_address(&self.interface)?;
        let group = multicast_group(task.id);
        let total_bytes = files.iter().map(|f| f.size).sum();
        let steps = files.len();
        let mut expected = participants.len();
        let mut bytes_before = 0;

        for (index, file) in files.into_iter().enumerate() {
            let running = self.running(task.id).await?.len();
            if running == 0 {
                return Ok(());
            }
            expected = expected.min(running);
            let step = TransferStep {
                task_id: task.id,
                step: index + 1,
                steps,
                bytes_before,
                total_bytes,
            };
            info!(
                task_id = task.id,
                step = step.step,
                steps,
                expected,
                "multicasting file"
            );
            let sending = bind_sender(local_ip, group, file.port, expected)
                .await?
                .send_file(file.path);
            match self.follow(sending, step, &targets, woken).await? {
                Some(joined) => expected = joined,
                None => return Ok(()),
            }
            bytes_before += file.size;
        }
        Ok(())
    }

    async fn follow(
        &self,
        mut sending: Sending,
        step: TransferStep,
        targets: &Targets,
        woken: &mut watch::Receiver<()>,
    ) -> Result<Option<usize>> {
        let mut progress = sending.progress();
        let mut log = tokio::time::interval_at(
            Instant::now() + PROGRESS_LOG_INTERVAL,
            PROGRESS_LOG_INTERVAL,
        );
        let mut joined = 0;
        let result = loop {
            tokio::select! {
                biased;
                result = &mut sending => break result,
                Ok(()) = woken.changed() => {
                    if !self.still_running(step.task_id).await {
                        return Ok(None);
                    }
                }
                Ok(()) = progress.changed() => {
                    let state = progress.borrow_and_update();
                    joined = joined.max(state.receivers.len());
                    self.progress.send_replace(Some(step.progress(&state)));
                }
                _ = log.tick() => log_progress(&progress.borrow(), step.task_id, targets),
            }
        };
        let joined = joined.max(progress.borrow().receivers.len());
        match result {
            Ok(()) => Ok(Some(joined)),
            Err(ProtoError::TransferIncomplete {
                complete,
                participants,
            }) => {
                warn!(
                    task_id = step.task_id,
                    complete, participants, "not every receiver completed the file"
                );
                Ok(Some(complete))
            }
            Err(e) => Err(AppError::Internal(format!("multicast send failed: {e}"))),
        }
    }

    async fn still_running(&self, task_id: i64) -> bool {
        match self.running(task_id).await {
            Ok(running) => !running.is_empty(),
            Err(e) => {
                warn!(task_id, err = %e, "could not check the multicast task, keeping it running");
                true
            }
        }
    }

    async fn running(&self, task_id: i64) -> Result<Vec<i64>> {
        Ok(self
            .tasks
            .get(task_id)
            .await?
            .hosts_in(TaskState::Running)
            .collect())
    }

    async fn files(&self, image_id: i64) -> Result<Vec<File>> {
        let partitions = self.images.get_partitions(image_id).await?;
        let slots = std::iter::once((self.image_service.get_partition_table_path(image_id), 0))
            .chain(partitions.into_iter().map(|p| {
                (
                    self.image_service
                        .get_partition_path(image_id, p.partition_number),
                    p.partition_number,
                )
            }));
        let mut files = Vec::new();
        for (path, slot) in slots {
            let size = tokio::fs::metadata(&path)
                .await
                .map_err(|e| AppError::Internal(format!("cannot read image file {path}: {e}")))?
                .len();
            files.push(File {
                path: path.into(),
                port: get_multicast_port(slot),
                size,
            });
        }
        Ok(files)
    }
}

async fn bind_sender(
    local_ip: Ipv4Addr,
    group: Ipv4Addr,
    port: u16,
    expected: usize,
) -> Result<Sender> {
    let deadline = Instant::now() + PORT_RELEASE_WAIT;
    loop {
        match Sender::builder().socket(local_ip, group, port) {
            Ok(builder) => {
                return Ok(builder
                    .min_receivers(expected)
                    .max_wait(MULTICAST_JOIN_WINDOW)
                    .build());
            }
            Err(ProtoError::Socket(e))
                if e.kind() == std::io::ErrorKind::AddrInUse && Instant::now() < deadline =>
            {
                tokio::time::sleep(PORT_RETRY_INTERVAL).await
            }
            Err(e) => {
                return Err(AppError::Internal(format!(
                    "failed to bind multicast sender: {e}"
                )));
            }
        }
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

struct Targets(HashMap<IpAddr, String>);

impl Targets {
    fn new(hosts: Vec<Host>, participants: &[i64]) -> Self {
        Self(
            hosts
                .into_iter()
                .filter(|h| participants.contains(&h.id))
                .filter_map(|h| {
                    Some((
                        h.ip.as_deref()?.parse().ok()?,
                        format!("{} (#{})", h.name, h.id),
                    ))
                })
                .collect(),
        )
    }

    fn name(&self, receiver: &ReceiverState) -> String {
        let ip = receiver.address.ip();
        self.0.get(&ip).cloned().unwrap_or_else(|| ip.to_string())
    }
}

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
            eta: Duration::try_from_secs_f64(left as f64 / bytes_per_second).ok(),
        }
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

fn limiting_receiver(state: &TransferState) -> Option<&ReceiverState> {
    let id = match state.limiting {
        LimitingFactor::WindowStalled { blocked_by, .. } => blocked_by,
        LimitingFactor::RateLimited { worst, .. } => worst,
        LimitingFactor::SinkStalled { receiver, .. } => receiver,
        _ => return None,
    };
    state.receivers.iter().find(|r| r.receiver_id == id)
}

fn log_progress(state: &TransferState, task_id: i64, targets: &Targets) {
    let culprit = limiting_receiver(state);
    info!(
        task_id,
        phase = %state.phase,
        mbit_per_second = state.bytes_per_second() * 8.0 / 1e6,
        percent_complete = ?state.fraction_complete().map(|f| f * 100.0),
        blocks_sent = state.blocks_sent,
        receivers = state.receivers.len(),
        parity_shards = state.parity_shards,
        limiting = %state.limiting,
        limiting_host = ?culprit.map(|r| targets.name(r)),
        limiting_host_loss = ?culprit.map(|r| r.unrecovered_loss),
        limiting_host_stall_ms = ?culprit.map(|r| r.sink_stall_ms),
        slowest_host = ?state.slowest().map(|r| (targets.name(r), r.slices_behind)),
        "multicast transfer progress"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::di::DIContainer;
    use imaged_shared::ServerEvent;
    use scuttlecast::receiver::Receiver;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::task::JoinHandle;

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

    fn host(id: i64, ip: &str) -> Host {
        Host::new(
            id,
            format!("lab-{id:02}"),
            format!("mac-{id}"),
            0,
            Some(ip.into()),
        )
    }

    #[test]
    fn receivers_are_named_after_the_participant_at_their_address() {
        let targets = Targets::new(vec![host(7, "10.0.0.1"), host(9, "10.0.0.9")], &[7]);

        assert_eq!(
            targets.name(&receiver_state(1, "10.0.0.1:5000")),
            "lab-07 (#7)"
        );
        assert_eq!(
            targets.name(&receiver_state(2, "10.0.0.9:5000")),
            "10.0.0.9"
        );
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

    #[test]
    fn the_loopback_interface_resolves_to_its_ipv4_address() {
        assert_eq!(interface_address("lo").unwrap(), Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn an_unknown_interface_is_an_error_rather_than_a_silent_default() {
        assert!(interface_address("definitely-not-an-interface").is_err());
    }

    static SESSIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static DB_ID: AtomicU64 = AtomicU64::new(0);
    const SETTLE: Duration = Duration::from_secs(20);

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

    async fn db_host(c: &DIContainer, mac: &str) -> i64 {
        c.host_repo
            .upsert_host(mac.to_string(), 1_000_000, None)
            .await
            .unwrap()
            .id
    }

    async fn image(c: &DIContainer, parttable: &[u8], partition: &[u8]) -> i64 {
        let image = c
            .image_repo
            .create_image(format!("cast-{}", DB_ID.fetch_add(1, Ordering::Relaxed)))
            .await
            .unwrap();
        c.image_repo
            .save_partition(image.id, 1, "ext4", partition.len() as i64)
            .await
            .unwrap();
        let table = PathBuf::from(c.image_service.get_partition_table_path(image.id));
        std::fs::create_dir_all(table.parent().unwrap()).unwrap();
        std::fs::write(&table, parttable).unwrap();
        std::fs::write(c.image_service.get_partition_path(image.id, 1), partition).unwrap();
        c.image_repo.mark_finished(image.id).await.unwrap();
        image.id
    }

    fn receive(task_id: i64, slot: i64) -> JoinHandle<Vec<u8>> {
        let receiver = Receiver::builder()
            .socket(
                Ipv4Addr::LOCALHOST,
                multicast_group(task_id),
                get_multicast_port(slot),
            )
            .expect("bind receiver")
            .max_wait(SETTLE)
            .build();
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_session_delivers_every_file_and_fails_only_hosts_that_disconnected() {
        let _session = SESSIONS.lock().await;
        let (c, _dir) = container().await;
        let connected = db_host(&c, "aa:bb:cc:dd:ee:01").await;
        let gone = db_host(&c, "aa:bb:cc:dd:ee:02").await;
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
            Ok(Some(ServerEvent::Task(t))) => assert_eq!(t.id, task.id),
            other => panic!("expected the task at session start, got {other:?}"),
        }
        let [table_a, table_b, data_a, data_b] = receivers;
        assert_eq!(table_a.await.unwrap(), parttable);
        assert_eq!(table_b.await.unwrap(), parttable);
        assert_eq!(data_a.await.unwrap(), partition);
        assert_eq!(data_b.await.unwrap(), partition);

        eventually(&c, task.id, gone, TaskState::Failed).await;
        assert_eq!(state_of(&c, task.id, connected).await, TaskState::Running);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelling_a_gathering_session_moves_on_to_the_next_task_in_queue_order() {
        let _session = SESSIONS.lock().await;
        let (c, _dir) = container().await;
        let first_host = db_host(&c, "aa:bb:cc:dd:ee:03").await;
        let second_host = db_host(&c, "aa:bb:cc:dd:ee:04").await;
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
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_host_busy_with_another_task_is_failed_instead_of_left_running() {
        let _session = SESSIONS.lock().await;
        let (c, _dir) = container().await;
        let busy = db_host(&c, "aa:bb:cc:dd:ee:07").await;
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
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn startup_fails_hosts_that_were_mid_transfer_but_keeps_queued_ones() {
        let _session = SESSIONS.lock().await;
        let (c, _dir) = container().await;
        let interrupted = db_host(&c, "aa:bb:cc:dd:ee:05").await;
        let queued = db_host(&c, "aa:bb:cc:dd:ee:06").await;
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
            "lo".to_string(),
        )
        .await
        .unwrap();

        let rows = c.task_repo.get(task.id).await.unwrap();
        let row = |id: i64| rows.hosts.iter().find(|h| h.host_id == id).unwrap();
        assert_eq!(row(interrupted).state, TaskState::Failed);
        assert_eq!(row(interrupted).error.as_deref(), Some(ORPHANED));
        assert_ne!(row(queued).state, TaskState::Failed);
    }
}
