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
    domain::task::TaskRepository,
    progress::ProgressBoard,
    registry::Registration,
};
use imaged_model::{
    AgentEvent, ServerEvent, Task,
    agent::StreamQuery,
    error::{AppError, Result},
};
use tokio::time::{Instant, MissedTickBehavior};

use crate::AgentInfo;

#[injectable::inject(host_repo: HostRepo, task_repo: TaskRepo, registry: Registry, liveness: Liveness, progress: Progress)]
pub async fn start_stream(
    agent: AgentInfo,
    Query(query): Query<StreamQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    let disk = match (query.disk_size_bytes, query.disk_error) {
        (Some(size), None) => Ok(size),
        (None, Some(reason)) => Err(reason),
        _ => {
            return Err(AppError::InvalidArgument(
                "report exactly one of disk_size_bytes and disk_error".into(),
            ));
        }
    };
    let host = host_repo.upsert_host(agent.mac, disk, agent.ip).await?;
    let stream = HostStream {
        host_id: host.id,
        registration: registry.register(host.id),
        current: current_task(&**task_repo, host.id).await?,
        tasks: task_repo,
        liveness: *liveness,
    };
    let progress = progress.clone();

    Ok(upgrade.on_upgrade(move |socket| async move {
        let host_id = stream.host_id;
        stream.serve(socket, &progress).await;
        progress.forget(host_id);
    }))
}

async fn current_task(tasks: &dyn TaskRepository, host_id: i64) -> Result<Option<Task>> {
    Ok(tasks
        .get_next(host_id)
        .await?
        .filter(|task| task.is_dispatchable_to(host_id))
        .map(|task| Task::new(task.id, task.task_type.into(), task.image_id)))
}

struct HostStream {
    host_id: i64,
    registration: Registration,
    current: Option<Task>,
    tasks: TaskRepo,
    liveness: AgentLiveness,
}

impl HostStream {
    async fn serve(mut self, mut socket: WebSocket, progress: &ProgressBoard) {
        if send(&mut socket, self.current).await.is_err() {
            return;
        }

        let host_id = self.host_id;
        let mut last_seen = Instant::now();
        let mut ping = tokio::time::interval(self.liveness.ping_interval);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            let asked = tokio::select! {
                biased;
                frame = socket.recv() => match frame {
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(message)) => {
                        last_seen = Instant::now();
                        let Message::Text(text) = message else { continue };
                        match serde_json::from_str(text.as_str()) {
                            Ok(AgentEvent::Idle) => true,
                            Ok(AgentEvent::Progress(report)) => {
                                progress.report(host_id, report);
                                continue;
                            }
                            Err(e) => {
                                tracing::warn!(host_id, err = %e, "ignoring an undecodable agent event");
                                continue;
                            }
                        }
                    }
                },
                dispatch = self.registration.receiver.recv() => match dispatch {
                    Some(()) => false,
                    None => break,
                },
                _ = ping.tick() => {
                    if last_seen.elapsed() > self.liveness.timeout {
                        tracing::info!(host_id, "agent stream liveness timeout; closing");
                        break;
                    }
                    if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                        break;
                    }
                    continue;
                }
            };
            let current = match current_task(&**self.tasks, host_id).await {
                Ok(current) => current,
                Err(e) => {
                    tracing::warn!(host_id, err = %e, "could not look up the host's current task; closing");
                    break;
                }
            };
            if (asked || current != self.current) && send(&mut socket, current).await.is_err() {
                break;
            }
            self.current = current;
        }
    }
}

async fn send(
    socket: &mut WebSocket,
    current: Option<Task>,
) -> std::result::Result<(), axum::Error> {
    let json = serde_json::to_string(&ServerEvent::Current(current)).map_err(axum::Error::new)?;
    socket.send(Message::text(json)).await
}

#[injectable::inject(host_repo: HostRepo, registry: Registry)]
pub async fn disconnect(agent: AgentInfo) -> Result {
    let host = host_repo.get_by_mac(&agent.mac).await?;
    registry.deregister(host.id);
    Ok(())
}
