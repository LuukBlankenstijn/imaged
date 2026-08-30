use crate::{
    domain::task::{Task, TaskState, TaskType},
    error::{AppError, Result},
};
use imaged_shared::{MULTICAST_DATA_ADDRESS, MULTICAST_RVD_ADDRESS, get_multicast_port};
use std::{
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
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
            handle,
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

        upd_sender(
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
            upd_sender(
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

async fn upd_sender(file: &str, portbase: u16, num_receivers: usize, interface: &str) -> Result {
    let rvd_address = MULTICAST_RVD_ADDRESS;
    let data_address = MULTICAST_DATA_ADDRESS;
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
            interface,
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

    fn build_manager(task_repo: Arc<FakeTaskRepo>, image_repo: Arc<FakeImageRepo>) -> MulticastManager {
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
            handle,
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
            r.handle.abort();
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

        repo.queue
            .lock()
            .unwrap()
            .push_back(mk_task(2, TaskType::Multicast, TaskState::Pending, None));
        manager.notify_new(2).unwrap();

        repo.release_park.store(true, Ordering::SeqCst);
        assert!(wait_for(|| manager.current.lock().unwrap().is_none()).await);

        assert!(
            wait_for(|| repo.get_ids.lock().unwrap().contains(&2)).await,
            "task 2 was dropped on the floor: no worker ever ran do_work for it"
        );
    }
}
