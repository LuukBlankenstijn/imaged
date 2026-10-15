use dioxus::fullstack::ServerEvents;
use dioxus::prelude::*;

use crate::model::{ConnectionChange, TaskProgress};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::{Progress, Registry};

#[cfg(feature = "server")]
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

#[get("/api/ui/connection-state")]
#[inject(registry: Registry)]
pub async fn connection_state() -> ServerFnResult<ServerEvents<ConnectionChange>> {
    let mut feed = registry.watch();
    Ok(ServerEvents::new(move |mut tx| async move {
        while let Some(change) = feed.next().await {
            if tx.send(change).await.is_err() {
                return;
            }
        }
    }))
}

#[get("/api/ui/task-progress")]
#[inject(progress: Progress)]
pub async fn task_progress() -> ServerFnResult<ServerEvents<Vec<TaskProgress>>> {
    let mut snapshots = progress.watch();
    Ok(ServerEvents::new(move |mut tx| async move {
        loop {
            let tasks = snapshots.borrow_and_update().tasks();
            if tx
                .send(tasks.into_iter().map(Into::into).collect())
                .await
                .is_err()
            {
                return;
            }
            tokio::time::sleep(PROGRESS_INTERVAL).await;
            if snapshots.changed().await.is_err() {
                return;
            }
        }
    }))
}
