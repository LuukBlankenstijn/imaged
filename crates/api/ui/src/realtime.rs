use dioxus::fullstack::ServerEvents;
use dioxus::prelude::*;

use crate::model::{ConnectionUpdate, MulticastProgress};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::{MulticastMgr, Registry};

#[get("/api/ui/connection-state")]
#[inject(registry: Registry)]
pub async fn connection_state() -> ServerFnResult<ServerEvents<ConnectionUpdate>> {
    let mut feed = registry.watch();
    Ok(ServerEvents::new(move |mut tx| async move {
        while let Some(change) = feed.next().await {
            if tx.send(change.into()).await.is_err() {
                return;
            }
        }
    }))
}

#[get("/api/ui/multicast-progress")]
#[inject(multicast: MulticastMgr)]
pub async fn multicast_progress() -> ServerFnResult<ServerEvents<Option<MulticastProgress>>> {
    let mut progress = multicast.progress();
    Ok(ServerEvents::new(move |mut tx| async move {
        loop {
            let current = progress.borrow_and_update().clone().map(Into::into);
            if tx.send(current).await.is_err() {
                return;
            }
            if progress.changed().await.is_err() {
                return;
            }
        }
    }))
}
