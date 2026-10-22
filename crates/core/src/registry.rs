use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use imaged_model::host::{ConnectionChange, HostConnectionEvent};
use tokio::sync::{broadcast, mpsc};

pub struct Registration {
    pub receiver: mpsc::Receiver<()>,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

pub struct ConnectionFeed {
    registry: Arc<HostRegistry>,
    updates: broadcast::Receiver<HostConnectionEvent>,
    connected: Option<Vec<i64>>,
}

impl ConnectionFeed {
    /// Resubscribes with a fresh snapshot whenever it falls behind, so a
    /// consumer applying every change ends up with the registry's state.
    pub async fn next(&mut self) -> Option<ConnectionChange> {
        loop {
            if let Some(connected) = self.connected.take() {
                return Some(ConnectionChange::Connected(connected));
            }
            match self.updates.recv().await {
                Ok(event) => return Some(ConnectionChange::Changed(event)),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let resynced = self.registry.watch();
                    *self = resynced;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

struct HostEntry {
    /// Monotonic id identifying this specific connection, used so a stale
    /// connection's cleanup can't tear down a newer registration for the host.
    generation: u64,
    dispatch: mpsc::Sender<()>,
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
    pub fn register(self: &Arc<Self>, id: i64) -> Registration {
        let mut hosts = self.hosts.write();
        let generation = hosts.next_generation;
        hosts.next_generation += 1;

        let (dispatch, receiver) = mpsc::channel(1);
        hosts.map.insert(
            id,
            HostEntry {
                generation,
                dispatch,
            },
        );
        let _ = self.broadcast.send(HostConnectionEvent::new(id, true));
        tracing::debug!(id, generation, "registered host");

        let hub = Arc::clone(self);
        let cleanup = move || {
            let mut hosts = hub.hosts.write();
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

        Registration {
            receiver,
            cleanup: Some(Box::new(cleanup)),
        }
    }

    /// Explicitly drop a host's connection because the host told us it is about
    /// to disconnect (e.g. it is rebooting).
    pub fn deregister(&self, id: i64) {
        let mut hosts = self.hosts.write();
        if hosts.map.remove(&id).is_some() {
            tracing::debug!(id, "host requested disconnect");
            let _ = self.broadcast.send(HostConnectionEvent::new(id, false));
        }
    }

    pub fn disconnect_all(&self) {
        let mut hosts = self.hosts.write();
        for id in hosts.map.drain().map(|(id, _)| id) {
            tracing::debug!(id, "disconnecting host");
            let _ = self.broadcast.send(HostConnectionEvent::new(id, false));
        }
    }

    pub fn dispatch(&self, host_id: i64) {
        let hosts = self.hosts.read();
        if let Some(entry) = hosts.map.get(&host_id) {
            let _ = entry.dispatch.try_send(());
        }
    }

    pub fn connected_hosts(&self) -> Vec<i64> {
        self.hosts.read().map.keys().copied().collect()
    }

    /// Subscribes with no gap: changes broadcast under the write lock.
    pub fn watch(self: &Arc<Self>) -> ConnectionFeed {
        let hosts = self.hosts.read();
        ConnectionFeed {
            registry: Arc::clone(self),
            updates: self.broadcast.subscribe(),
            connected: Some(hosts.map.keys().copied().collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionFeed, HostRegistry, Registration};
    use imaged_model::host::{ConnectionChange, HostConnectionEvent};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc::error::TryRecvError;

    async fn watch(registry: &Arc<HostRegistry>, connected: &[i64]) -> ConnectionFeed {
        let mut feed = registry.watch();
        match feed.next().await {
            Some(ConnectionChange::Connected(mut hosts)) => {
                hosts.sort_unstable();
                assert_eq!(hosts, connected);
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }
        feed
    }

    async fn change(feed: &mut ConnectionFeed) -> HostConnectionEvent {
        match feed.next().await {
            Some(ConnectionChange::Changed(event)) => event,
            other => panic!("expected a change, got {other:?}"),
        }
    }

    async fn assert_quiet(feed: &mut ConnectionFeed) {
        let next = tokio::time::timeout(Duration::from_millis(50), feed.next()).await;
        assert!(next.is_err(), "unexpected change: {:?}", next.unwrap());
    }

    fn pending(registration: &mut Registration) -> Result<(), TryRecvError> {
        registration.receiver.try_recv()
    }

    #[tokio::test]
    async fn register_lists_the_host_broadcasts_a_connect_event_and_receives_dispatches() {
        let registry = Arc::new(HostRegistry::default());
        let mut feed = watch(&registry, &[]).await;

        let mut reg = registry.register(1);

        let event = change(&mut feed).await;
        assert_eq!(event.id, 1);
        assert!(event.connected);

        assert_eq!(registry.connected_hosts(), vec![1]);

        assert_eq!(pending(&mut reg), Err(TryRecvError::Empty));
        registry.dispatch(1);
        assert_eq!(pending(&mut reg), Ok(()));
        assert_eq!(pending(&mut reg), Err(TryRecvError::Empty));
    }

    #[tokio::test]
    async fn dispatches_that_arrive_before_the_connection_wakes_coalesce_into_one() {
        let registry = Arc::new(HostRegistry::default());
        let mut reg = registry.register(1);

        for _ in 0..5 {
            registry.dispatch(1);
        }

        assert_eq!(pending(&mut reg), Ok(()));
        assert_eq!(pending(&mut reg), Err(TryRecvError::Empty));
    }

    #[tokio::test]
    async fn a_dispatch_only_reaches_the_host_it_names() {
        let registry = Arc::new(HostRegistry::default());
        let mut one = registry.register(1);
        let mut two = registry.register(2);

        registry.dispatch(2);

        assert_eq!(pending(&mut one), Err(TryRecvError::Empty));
        assert_eq!(pending(&mut two), Ok(()));
    }

    #[tokio::test]
    async fn a_duplicate_register_displaces_the_old_connection_with_one_more_connect_and_no_disconnect()
     {
        let registry = Arc::new(HostRegistry::default());
        let mut feed = watch(&registry, &[]).await;

        let mut first = registry.register(1);
        assert!(change(&mut feed).await.connected);

        let mut second = registry.register(1);

        let redisplace = change(&mut feed).await;
        assert_eq!(redisplace.id, 1);
        assert!(redisplace.connected);
        assert_quiet(&mut feed).await;

        assert!(first.receiver.recv().await.is_none());

        registry.dispatch(1);
        assert_eq!(pending(&mut second), Ok(()));
        assert_quiet(&mut feed).await;
    }

    #[tokio::test]
    async fn a_stale_registration_drop_does_not_evict_a_newer_registration_for_the_same_host() {
        let registry = Arc::new(HostRegistry::default());

        let first = registry.register(1);
        let mut second = registry.register(1);

        drop(first);

        assert_eq!(registry.connected_hosts(), vec![1]);

        registry.dispatch(1);
        assert_eq!(pending(&mut second), Ok(()));
    }

    #[tokio::test]
    async fn deregister_removes_the_host_broadcasts_disconnect_and_is_a_noop_for_unknown_ids() {
        let registry = Arc::new(HostRegistry::default());
        let mut feed = watch(&registry, &[]).await;

        let mut reg = registry.register(2);
        assert!(change(&mut feed).await.connected);

        registry.deregister(2);
        let event = change(&mut feed).await;
        assert_eq!(event.id, 2);
        assert!(!event.connected);
        assert!(registry.connected_hosts().is_empty());
        assert!(reg.receiver.recv().await.is_none());

        registry.deregister(999);
        assert_quiet(&mut feed).await;
    }

    #[tokio::test]
    async fn disconnect_all_ends_every_host_stream_and_broadcasts_each_disconnect() {
        let registry = Arc::new(HostRegistry::default());
        let mut first = registry.register(3);
        let mut second = registry.register(4);
        let mut feed = watch(&registry, &[3, 4]).await;

        registry.disconnect_all();

        assert!(first.receiver.recv().await.is_none());
        assert!(second.receiver.recv().await.is_none());
        let mut disconnected = [change(&mut feed).await, change(&mut feed).await];
        disconnected.sort_unstable_by_key(|event| event.id);
        assert_eq!(
            disconnected
                .iter()
                .map(|event| (event.id, event.connected))
                .collect::<Vec<_>>(),
            [(3, false), (4, false)]
        );
        assert!(registry.connected_hosts().is_empty());

        drop(first);
        assert_quiet(&mut feed).await;
    }

    #[tokio::test]
    async fn a_dispatch_for_an_absent_host_is_a_silent_noop() {
        let registry = Arc::new(HostRegistry::default());
        registry.dispatch(999);
        assert!(registry.connected_hosts().is_empty());
    }

    #[tokio::test]
    async fn dropping_a_live_registration_removes_the_host_and_broadcasts_disconnect() {
        let registry = Arc::new(HostRegistry::default());
        let mut feed = watch(&registry, &[]).await;

        let reg = registry.register(4);
        assert!(change(&mut feed).await.connected);

        drop(reg);

        let event = change(&mut feed).await;
        assert_eq!(event.id, 4);
        assert!(!event.connected);
        assert!(registry.connected_hosts().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_register_and_deregister_churn_never_poisons_the_lock_and_ends_empty() {
        let registry = Arc::new(HostRegistry::default());
        let mut handles = Vec::new();
        for host in 0..8i64 {
            for _ in 0..4 {
                let registry = Arc::clone(&registry);
                handles.push(tokio::spawn(async move {
                    for _ in 0..50 {
                        let reg = registry.register(host);
                        registry.dispatch(host);
                        registry.deregister(host);
                        drop(reg);
                    }
                }));
            }
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert!(registry.connected_hosts().is_empty());

        let mut reg = registry.register(0);
        registry.dispatch(0);
        assert!(reg.receiver.recv().await.is_some());
    }

    #[tokio::test]
    async fn watch_hands_out_the_connected_hosts_and_only_the_changes_after_them() {
        let registry = Arc::new(HostRegistry::default());
        let _first = registry.register(1);

        let mut feed = watch(&registry, &[1]).await;
        assert_quiet(&mut feed).await;

        let second = registry.register(2);
        let connect = change(&mut feed).await;
        assert_eq!(connect.id, 2);
        assert!(connect.connected);

        drop(second);
        let disconnect = change(&mut feed).await;
        assert_eq!(disconnect.id, 2);
        assert!(!disconnect.connected);
        assert_eq!(registry.connected_hosts(), vec![1]);
    }

    #[tokio::test]
    async fn a_feed_that_falls_behind_is_resynced_with_a_fresh_snapshot() {
        let registry = Arc::new(HostRegistry::default());
        let mut feed = watch(&registry, &[]).await;

        let registrations: Vec<_> = (0..64).map(|id| registry.register(id)).collect();

        match feed.next().await {
            Some(ConnectionChange::Connected(mut hosts)) => {
                hosts.sort_unstable();
                assert_eq!(hosts, (0..64).collect::<Vec<_>>());
            }
            other => panic!("expected a resync snapshot, got {other:?}"),
        }
        drop(registrations);
    }
}
