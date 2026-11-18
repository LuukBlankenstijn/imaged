use axum::{
    extract::{
        Query, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use std::collections::HashMap;

use bytes::Bytes;
use imaged_core::{
    AgentLiveness,
    di::{HostRepo, Liveness, Progress, Registry, TaskRepo},
    domain::task::TaskRepository,
    progress::ProgressBoard,
    registry::{PendingCommand, Registration},
};
use imaged_model::{
    AgentEvent, ServerEvent, Task,
    agent::StreamQuery,
    command::{CommandOutcome, CommandReport, CommandRequest},
    error::{AppError, Result},
};
use tokio::sync::oneshot;
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
        hosts: host_repo,
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
    while let Some(task) = tasks
        .get_next(host_id)
        .await?
        .filter(|task| task.is_dispatchable_to(host_id))
    {
        match task.work(host_id) {
            Ok(work) => return Ok(Some(Task::new(task.id, work))),
            Err(e) => {
                tracing::warn!(host_id, task_id = task.id, err = %e, "failing a task the host cannot run");
                tasks.mark_failed(task.id, host_id, &e.to_string()).await?;
            }
        }
    }
    Ok(None)
}

struct HostStream {
    host_id: i64,
    registration: Registration,
    current: Option<Task>,
    tasks: TaskRepo,
    hosts: HostRepo,
    liveness: AgentLiveness,
}

impl HostStream {
    async fn serve(mut self, mut socket: WebSocket, progress: &ProgressBoard) {
        if send(&mut socket, ServerEvent::Current(self.current.clone()))
            .await
            .is_err()
        {
            return;
        }

        let host_id = self.host_id;
        let mut last_seen = Instant::now();
        let mut ping = tokio::time::interval(self.liveness.ping_interval);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut pending: HashMap<u64, oneshot::Sender<CommandOutcome>> = HashMap::new();
        let mut next_command = 0;

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
                            Ok(AgentEvent::Partitions(partitions)) => {
                                if let Err(e) = self.hosts.set_partitions(host_id, &partitions).await {
                                    tracing::warn!(host_id, err = %e, "could not store the host's partitions");
                                }
                                continue;
                            }
                            Ok(AgentEvent::Command(CommandReport { id, outcome })) => {
                                if let Some(reply) = pending.remove(&id) {
                                    let _ = reply.send(outcome);
                                }
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
                Some(PendingCommand { script, reply }) = self.registration.commands.recv() => {
                    let id = next_command;
                    next_command += 1;
                    if send(&mut socket, ServerEvent::Run(CommandRequest { id, script })).await.is_err() {
                        break;
                    }
                    pending.retain(|_, waiting| !waiting.is_closed());
                    pending.insert(id, reply);
                    continue;
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
            if (asked || current != self.current)
                && send(&mut socket, ServerEvent::Current(current.clone()))
                    .await
                    .is_err()
            {
                break;
            }
            self.current = current;
        }
    }
}

async fn send(socket: &mut WebSocket, event: ServerEvent) -> std::result::Result<(), axum::Error> {
    let json = serde_json::to_string(&event).map_err(axum::Error::new)?;
    socket.send(Message::text(json)).await
}

#[injectable::inject(host_repo: HostRepo, registry: Registry)]
pub async fn disconnect(agent: AgentInfo) -> Result {
    let host = host_repo.get_by_mac(&agent.mac).await?;
    registry.deregister(host.id);
    Ok(())
}
