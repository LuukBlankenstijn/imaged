use dioxus::fullstack::ServerEvents;
use dioxus::prelude::*;

use crate::model::HostConnectionEvent;

#[cfg(feature = "server")]
use inject::inject;

#[cfg(feature = "server")]
use imaged_server_core::di::Registry;

#[get("/api/connection-state")]
#[inject(registry: Registry)]
pub async fn connection_state() -> ServerFnResult<ServerEvents<HostConnectionEvent>> {
    let initial = registry.get_current_state();
    let mut updates = registry.subscribe_state();
    Ok(ServerEvents::new(move |mut tx| async move {
        for event in initial {
            if tx.send(event.into()).await.is_err() {
                return;
            }
        }
        while let Ok(update) = updates.recv().await {
            if tx.send(update.into()).await.is_err() {
                break;
            }
        }
    }))
}
