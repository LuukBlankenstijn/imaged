use crate::{
    domain::task::{Task, TaskState, TaskType},
    error::{AppError, Result},
};
use imaged_shared::{MULTICAST_GROUP_ADDRESS, get_multicast_port};
use std::{
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
    domain::{image::ImageRepository, task::TaskRepository},
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

#[derive(Clone)]
pub struct MulticastManager {
    task_repo: Arc<dyn TaskRepository>,
    image_repo: Arc<dyn ImageRepository>,
    image_service: Arc<ImageService>,
    interface: String,
    current: Arc<Mutex<Option<RunningMulticastTask>>>,
    next_generation: Arc<AtomicU64>,
}

impl MulticastManager {
    pub async fn new(
        task_repo: Arc<dyn TaskRepository>,
        image_repo: Arc<dyn ImageRepository>,
        image_service: Arc<ImageService>,
        interface: String,
    ) -> Result<Self> {
        let new = Self {
            task_repo: task_repo.clone(),
            image_repo,
            image_service,
            interface,
            current: Arc::new(Mutex::new(None)),
            next_generation: Arc::new(AtomicU64::new(0)),
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

        let partition_table_path = self.image_service.get_partition_table_path(image_id);

        send_file(
            &partition_table_path,
            get_multicast_port(0),
            num_receivers,
            &self.interface,
        )
        .await?;

        let partitions = self.image_repo.get_partitions(image_id).await?;
        for p in partitions.into_iter() {
            info!(task_id=%task.id, image_id=%image_id, partition_number=%p.partition_number, "sending partition over multicast");
            let partition_path = self
                .image_service
                .get_partition_path(image_id, p.partition_number);
            send_file(
                &partition_path,
                get_multicast_port(p.partition_number),
                num_receivers,
                &self.interface,
            )
            .await?;
        }

        Ok(())
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

fn limiting_receiver(state: &TransferState) -> Option<&ReceiverState> {
    let id = match state.limiting {
        LimitingFactor::WindowStalled { blocked_by, .. } => blocked_by,
        LimitingFactor::RateLimited { worst, .. } => worst,
        LimitingFactor::SinkStalled { receiver, .. } => receiver,
        _ => return None,
    };
    state.receivers.iter().find(|r| r.receiver_id == id)
}

async fn log_progress(progress: watch::Receiver<TransferState>) {
    let mut ticker = tokio::time::interval(PROGRESS_LOG_INTERVAL);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let state = progress.borrow();
        let culprit = limiting_receiver(&state);
        info!(
            mbit_per_second = state.bytes_per_second() * 8.0 / 1e6,
            blocks_sent = state.blocks_sent,
            receivers = state.receivers.len(),
            parity_shards = state.parity_shards,
            draining = state.draining,
            limiting = %state.limiting,
            limiting_host = ?culprit.map(|r| r.address),
            limiting_host_loss = ?culprit.map(|r| r.unrecovered_loss),
            limiting_host_stall_ms = ?culprit.map(|r| r.sink_stall_ms),
            slowest_host = ?state.slowest().map(|r| (r.address, r.slices_behind)),
            "multicast transfer progress"
        );
    }
}

async fn send_file(file: &str, port: u16, num_receivers: usize, interface: &str) -> Result {
    let sender = Sender::builder()
        .socket(interface_address(interface)?, MULTICAST_GROUP_ADDRESS, port)
        .map_err(|e| AppError::Internal(format!("failed to bind multicast sender: {e}")))?
        .min_receivers(num_receivers)
        .build();

    let _reporter = AbortOnDropHandle::new(tokio::spawn(log_progress(sender.progress())));

    sender
        .send_file(PathBuf::from(file))
        .await
        .map_err(|e| AppError::Internal(format!("multicast send of {file} failed: {e}")))
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
                    res = sender.send_stream(std::io::Cursor::new(payload)) => {
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

        send_file(path.to_str().expect("utf8 path"), PORT, 1, "lo")
            .await
            .expect("send");

        let (received, summary) = receiving.await.expect("receive task");
        let _ = std::fs::remove_file(&path);

        assert_eq!(received, payload);
        assert_eq!(summary.total_bytes, payload.len() as u64);
    }

    use crate::di::DIContainer;
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

    fn build_manager(
        task_repo: Arc<FakeTaskRepo>,
        image_repo: Arc<FakeImageRepo>,
    ) -> MulticastManager {
        MulticastManager {
            task_repo: task_repo as Arc<dyn TaskRepository>,
            image_repo: image_repo as Arc<dyn ImageRepository>,
            image_service: Arc::new(ImageService::new("/tmp/imaged-mcmgr-unused".to_string())),
            interface: "lo".to_string(),
            current: Arc::new(Mutex::new(None)),
            next_generation: Arc::new(AtomicU64::new(0)),
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
