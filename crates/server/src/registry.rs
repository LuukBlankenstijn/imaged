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

impl HostRegistry {
    pub fn new() -> Self {
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
