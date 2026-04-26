use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use derive_more::Constructor;
use tokio::sync::{broadcast, mpsc};

use crate::domain::types::Task;
use crate::error::{AppError, Result};

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

pub struct HostRegistry {
    hosts: RwLock<HashMap<i64, mpsc::UnboundedSender<Task>>>,

    broadcast: broadcast::Sender<HostConnectionEvent>,
}

impl HostRegistry {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(32);
        Self {
            hosts: Default::default(),
            broadcast: sender,
        }
    }
}

impl HostRegistry {
    pub fn register(self: &Arc<Self>, id: i64) -> Result<Registration<Task>> {
        let mut admins = self.hosts.write().unwrap();
        if admins.contains_key(&id) {
            return Err(crate::error::AppError::FailedPrecondition(
                "host already connected".into(),
            ));
        }
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        admins.insert(id, command_tx);
        let _ = self.broadcast.send(HostConnectionEvent::new(id, true));
        tracing::debug!(id, "registered host");

        let hub = Arc::clone(self);
        let cleanup = move || {
            let mut admins = hub.hosts.write().unwrap();
            admins.remove(&id);
            tracing::debug!(id, "deregistered host");
            let _ = hub.broadcast.send(HostConnectionEvent::new(id, false));
        };

        Ok(Registration::new(command_rx, Some(Box::new(cleanup))))
    }

    pub fn send_task(&self, task: Task, id: i64) -> Result {
        let hosts = self.hosts.read().unwrap();
        if let Some(sender) = hosts.get(&id) {
            let _ = sender.send(task);
        }
        Err(AppError::NotFound("host not registered".to_string()))
    }

    pub fn subscribe_state(&self) -> broadcast::Receiver<HostConnectionEvent> {
        self.broadcast.subscribe()
    }

    // gets the current connection state as a set of diffs
    pub fn get_current_state(&self) -> Vec<HostConnectionEvent> {
        let hosts = self.hosts.read().unwrap();
        hosts
            .keys()
            .map(|k| HostConnectionEvent::new(*k, true))
            .collect()
    }
}
