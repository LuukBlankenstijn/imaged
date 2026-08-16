use dioxus_fullstack::*;
use imaged_shared::{ServerEvent, error::Result};

#[cfg(feature = "server")]
use crate::AgentInfo;
#[cfg(feature = "server")]
use imaged_core::di::{HostRepo, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_shared::Task;

#[injectable::inject(host_repo: HostRepo, task_repo: TaskRepo, registry: Registry)]
#[get("/api/client/stream?disk_size_bytes", agent: AgentInfo)]
pub async fn start_stream(disk_size_bytes: u64) -> Result<ServerEvents<ServerEvent>> {
    let host = host_repo
        .upsert_host(agent.mac.clone(), disk_size_bytes, agent.ip.clone())
        .await?;
    let mut registration = registry.register(host.id)?;
    let pending = task_repo.get_next(host.id).await?;

    Ok(ServerEvents::new(move |mut tx| async move {
        if let Some(task) = pending {
            let event = ServerEvent::from(Task::new(task.id, task.task_type.into(), task.image_id));
            if tx.send(event).await.is_err() {
                return;
            }
        }
        while let Some(event) = registration.receiver.recv().await {
            tracing::info!("sending event");
            if tx.send(event).await.is_err() {
                break;
            }
        }
    }))
}

#[injectable::inject(host_repo: HostRepo, registry: Registry)]
#[post("/api/client/stream/disconnect", agent: AgentInfo)]
pub async fn disconnect() -> Result {
    let host = host_repo.get_by_mac(&agent.mac).await?;
    registry.deregister(host.id);
    Ok(())
}
