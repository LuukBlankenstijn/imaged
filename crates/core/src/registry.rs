use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::error;

use derive_more::Constructor;
use imaged_shared::{ServerEvent, Task};
use tokio::sync::{broadcast, mpsc};

use crate::domain::task::Task as DomainTask;
use crate::error::Result;

#[derive(Constructor)]
pub struct Registration<T> {
    pub receiver: mpsc::UnboundedReceiver<T>,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
}

impl<T> Drop for Registration<T> {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

#[derive(Constructor, Debug, Clone)]
pub struct HostConnectionEvent {
    pub id: i64,
    pub connected: bool,
}

struct HostEntry {
    /// Monotonic id identifying this specific connection, used so a stale
    /// connection's cleanup can't tear down a newer registration for the host.
    generation: u64,
    sender: mpsc::UnboundedSender<ServerEvent>,
}

#[derive(Default)]
struct Hosts {
    map: HashMap<i64, HostEntry>,
    next_generation: u64,
}

pub struct HostRegistry {
    hosts: RwLock<Hosts>,

    broadcast: broadcast::Sender<HostConnectionEvent>,
}

impl Default for HostRegistry {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(32);
        Self {
            hosts: RwLock::new(Hosts::default()),
            broadcast: sender,
        }
    }
}

impl HostRegistry {
    pub fn register(self: &Arc<Self>, id: i64) -> Result<Registration<ServerEvent>> {
        let mut hosts = self.hosts.write().unwrap();
        if hosts.map.contains_key(&id) {
            error!("host {id} tried to register but was already registered");
            return Err(crate::error::AppError::FailedPrecondition(
                "host already connected".into(),
            ));
        }
        let generation = hosts.next_generation;
        hosts.next_generation += 1;

        let (command_tx, command_rx) = mpsc::unbounded_channel();
        hosts.map.insert(
            id,
            HostEntry {
                generation,
                sender: command_tx,
            },
        );
        let _ = self.broadcast.send(HostConnectionEvent::new(id, true));
        tracing::debug!(id, generation, "registered host");

        let hub = Arc::clone(self);
        let cleanup = move || {
            let mut hosts = hub.hosts.write().unwrap();
            // Only tear down if *this* connection is still the registered one.
            if hosts
                .map
                .get(&id)
                .is_some_and(|e| e.generation == generation)
            {
                hosts.map.remove(&id);
                tracing::debug!(id, generation, "deregistered host");
                let _ = hub.broadcast.send(HostConnectionEvent::new(id, false));
            } else {
                tracing::debug!(id, generation, "ignoring stale registration cleanup");
            }
        };

        Ok(Registration::new(command_rx, Some(Box::new(cleanup))))
    }

    /// Explicitly drop a host's connection because the host told us it is about
    /// to disconnect (e.g. it is rebooting).
    pub fn deregister(&self, id: i64) {
        let mut hosts = self.hosts.write().unwrap();
        if hosts.map.remove(&id).is_some() {
            tracing::debug!(id, "host requested disconnect");
            let _ = self.broadcast.send(HostConnectionEvent::new(id, false));
        }
    }

    pub fn cancel_task(&self, host_id: i64, task_id: i64) {
        let hosts = self.hosts.read().unwrap();
        if let Some(entry) = hosts.map.get(&host_id) {
            let _ = entry.sender.send(task_id.into());
        }
    }

    pub fn send_task(&self, host_id: i64, task: &DomainTask) {
        let hosts = self.hosts.read().unwrap();
        if let Some(entry) = hosts.map.get(&host_id) {
            let msg = Task::new(task.id, task.task_type.into(), task.image_id);
            let _ = entry.sender.send(msg.into());
        }
    }

    pub fn subscribe_state(&self) -> broadcast::Receiver<HostConnectionEvent> {
        self.broadcast.subscribe()
    }

    // gets the current connection state as a set of diffs
    pub fn get_current_state(&self) -> Vec<HostConnectionEvent> {
        let hosts = self.hosts.read().unwrap();
        hosts
            .map
            .keys()
            .map(|k| HostConnectionEvent::new(*k, true))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::HostRegistry;
    use crate::domain::task::{Task as DomainTask, TaskType};
    use crate::error::AppError;
    use chrono::Utc;
    use imaged_shared::ServerEvent;
    use std::sync::Arc;
    use tokio::sync::broadcast::error::TryRecvError;

    fn domain_task(id: i64, task_type: TaskType, image_id: Option<i64>) -> DomainTask {
        DomainTask {
            id,
            task_type,
            hosts: Vec::new(),
            image_id,
            image_name: None,
            image_deleted: false,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn register_delivers_tasks_lists_the_host_and_broadcasts_a_connect_event() {
        let registry = Arc::new(HostRegistry::default());
        let mut state_rx = registry.subscribe_state();

        let mut reg = registry.register(1).unwrap();

        let event = state_rx.recv().await.unwrap();
        assert_eq!(event.id, 1);
        assert!(event.connected);

        let state = registry.get_current_state();
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].id, 1);
        assert!(state[0].connected);

        registry.send_task(1, &domain_task(7, TaskType::Deploy, Some(3)));
        match reg.receiver.recv().await.unwrap() {
            ServerEvent::Task(t) => {
                assert_eq!(t.id, 7);
                assert_eq!(t.task_type, imaged_shared::TaskType::Deploy);
                assert_eq!(t.image_id, Some(3));
            }
            other => panic!("expected a task event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_duplicate_register_is_rejected_without_a_second_connect_event() {
        let registry = Arc::new(HostRegistry::default());
        let mut state_rx = registry.subscribe_state();

        let mut first = registry.register(1).unwrap();
        let connect = state_rx.recv().await.unwrap();
        assert!(connect.connected);

        assert!(matches!(
            registry.register(1),
            Err(AppError::FailedPrecondition(_))
        ));

        assert!(matches!(state_rx.try_recv(), Err(TryRecvError::Empty)));

        registry.send_task(1, &domain_task(9, TaskType::Capture, None));
        match first.receiver.recv().await.unwrap() {
            ServerEvent::Task(t) => assert_eq!(t.id, 9),
            other => panic!("expected a task event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_stale_registration_drop_does_not_evict_a_newer_registration_for_the_same_host() {
        let registry = Arc::new(HostRegistry::default());

        let first = registry.register(1).unwrap();
        registry.deregister(1);
        let mut second = registry.register(1).unwrap();

        drop(first);

        let state = registry.get_current_state();
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].id, 1);

        registry.send_task(1, &domain_task(7, TaskType::Deploy, Some(3)));
        match second.receiver.recv().await.unwrap() {
            ServerEvent::Task(t) => assert_eq!(t.id, 7),
            other => panic!("expected a task event on the newer receiver, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deregister_removes_the_host_broadcasts_disconnect_and_is_a_noop_for_unknown_ids() {
        let registry = Arc::new(HostRegistry::default());
        let mut state_rx = registry.subscribe_state();

        let _reg = registry.register(2).unwrap();
        assert!(state_rx.recv().await.unwrap().connected);

        registry.deregister(2);
        let event = state_rx.recv().await.unwrap();
        assert_eq!(event.id, 2);
        assert!(!event.connected);
        assert!(registry.get_current_state().is_empty());

        registry.deregister(999);
        assert!(matches!(state_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn send_and_cancel_for_an_absent_host_are_silent_noops() {
        let registry = Arc::new(HostRegistry::default());
        registry.send_task(999, &domain_task(1, TaskType::Deploy, None));
        registry.cancel_task(999, 1);
        assert!(registry.get_current_state().is_empty());
    }

    #[tokio::test]
    async fn cancel_and_send_deliver_the_expected_events_to_a_registered_host() {
        let registry = Arc::new(HostRegistry::default());
        let mut reg = registry.register(5).unwrap();

        registry.cancel_task(5, 42);
        match reg.receiver.recv().await.unwrap() {
            ServerEvent::Cancel(id) => assert_eq!(id, 42),
            other => panic!("expected a cancel event, got {other:?}"),
        }

        registry.send_task(5, &domain_task(9, TaskType::Capture, Some(11)));
        match reg.receiver.recv().await.unwrap() {
            ServerEvent::Task(t) => {
                assert_eq!(t.id, 9);
                assert_eq!(t.task_type, imaged_shared::TaskType::Capture);
                assert_eq!(t.image_id, Some(11));
            }
            other => panic!("expected a task event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dropping_a_live_registration_removes_the_host_and_broadcasts_disconnect() {
        let registry = Arc::new(HostRegistry::default());
        let mut state_rx = registry.subscribe_state();

        let reg = registry.register(4).unwrap();
        assert!(state_rx.recv().await.unwrap().connected);

        drop(reg);

        let event = state_rx.recv().await.unwrap();
        assert_eq!(event.id, 4);
        assert!(!event.connected);
        assert!(registry.get_current_state().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_register_and_deregister_churn_never_poisons_the_lock_and_ends_empty() {
        let registry = Arc::new(HostRegistry::default());
        let mut handles = Vec::new();
        for host in 0..32i64 {
            let registry = Arc::clone(&registry);
            handles.push(tokio::spawn(async move {
                for _ in 0..50 {
                    let reg = registry.register(host).unwrap();
                    registry.send_task(host, &domain_task(1, TaskType::Reboot, None));
                    registry.cancel_task(host, 1);
                    registry.deregister(host);
                    drop(reg);
                }
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert!(registry.get_current_state().is_empty());
    }
}
