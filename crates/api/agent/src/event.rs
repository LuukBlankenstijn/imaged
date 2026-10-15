use axum::{
    extract::{
        Query, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use bytes::Bytes;
use imaged_core::{
    AgentLiveness,
    di::{HostRepo, Liveness, Progress, Registry, TaskRepo},
    domain::task::Task,
    progress::ProgressBoard,
    registry::Registration,
};
use imaged_model::{AgentEvent, ServerEvent, agent::StreamQuery, error::Result};
use tokio::time::{Instant, MissedTickBehavior};

use crate::AgentInfo;

#[injectable::inject(host_repo: HostRepo, task_repo: TaskRepo, registry: Registry, liveness: Liveness, progress: Progress)]
pub async fn start_stream(
    agent: AgentInfo,
    Query(StreamQuery { disk_size_bytes }): Query<StreamQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    let host = host_repo
        .upsert_host(agent.mac, disk_size_bytes, agent.ip)
        .await?;
    let host_id = host.id;
    let registration = registry.register(host_id);
    let pending = task_repo
        .get_next(host_id)
        .await?
        .filter(|task| task.is_dispatchable_to(host_id));
    let liveness = *liveness;
    let progress = progress.clone();

    Ok(upgrade.on_upgrade(move |socket| async move {
        serve(socket, host_id, registration, pending, liveness, &progress).await;
        progress.forget(host_id);
    }))
}

async fn serve(
    mut socket: WebSocket,
    host_id: i64,
    mut registration: Registration<ServerEvent>,
    pending: Option<Task>,
    liveness: AgentLiveness,
    progress: &ProgressBoard,
) {
    if let Some(task) = pending {
        let task = imaged_model::Task::new(task.id, task.task_type.into(), task.image_id);
        if send(&mut socket, &ServerEvent::from(task)).await.is_err() {
            return;
        }
    }

    let mut last_seen = Instant::now();
    let mut ping = tokio::time::interval(liveness.ping_interval);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            event = registration.receiver.recv() => {
                let Some(event) = event else { break };
                if send(&mut socket, &event).await.is_err() {
                    break;
                }
            }
            frame = socket.recv() => match frame {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(message)) => {
                    last_seen = Instant::now();
                    if let Message::Text(text) = message {
                        receive(text.as_str(), host_id, progress);
                    }
                }
            },
            _ = ping.tick() => {
                if last_seen.elapsed() > liveness.timeout {
                    tracing::info!(host_id, "agent stream liveness timeout; closing");
                    break;
                }
                if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn receive(text: &str, host_id: i64, progress: &ProgressBoard) {
    match serde_json::from_str(text) {
        Ok(AgentEvent::Progress(report)) => progress.report(host_id, report),
        Err(e) => tracing::warn!(host_id, err = %e, "ignoring an undecodable agent event"),
    }
}

async fn send(socket: &mut WebSocket, event: &ServerEvent) -> std::result::Result<(), axum::Error> {
    let json = serde_json::to_string(event).map_err(axum::Error::new)?;
    socket.send(Message::text(json)).await
}

#[injectable::inject(host_repo: HostRepo, registry: Registry)]
pub async fn disconnect(agent: AgentInfo) -> Result {
    let host = host_repo.get_by_mac(&agent.mac).await?;
    registry.deregister(host.id);
    Ok(())
}
