use dioxus::fullstack::ServerEvents;
use dioxus::prelude::*;

use crate::model::ConnectionUpdate;

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::Registry;

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
