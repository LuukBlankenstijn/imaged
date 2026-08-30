use dioxus_fullstack::*;
use imaged_shared::{ServerEvent, error::Result};

#[cfg(feature = "server")]
use crate::AgentInfo;
#[cfg(feature = "server")]
use bytes::Bytes;
#[cfg(feature = "server")]
use imaged_core::di::{HostRepo, Liveness, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_shared::Task;

#[injectable::inject(host_repo: HostRepo, task_repo: TaskRepo, registry: Registry, liveness: Liveness)]
#[get("/api/client/stream?disk_size_bytes", agent: AgentInfo)]
pub async fn start_stream(
    disk_size_bytes: u64,
    options: WebSocketOptions,
) -> Result<Websocket<(), ServerEvent>> {
    let host = host_repo
        .upsert_host(agent.mac.clone(), disk_size_bytes, agent.ip.clone())
        .await?;
    let host_id = host.id;
    let mut registration = registry.register(host_id);
    let pending = task_repo.get_next(host_id).await?;
    let liveness = *liveness;

    Ok(options.on_upgrade(
        move |mut socket: TypedWebsocket<(), ServerEvent>| async move {
            if let Some(task) = pending {
                let event =
                    ServerEvent::from(Task::new(task.id, task.task_type.into(), task.image_id));
                if socket.send(event).await.is_err() {
                    return;
                }
            }

            let mut last_seen = tokio::time::Instant::now();
            let mut ping = tokio::time::interval(liveness.ping_interval);
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            enum Next {
                Send(ServerEvent),
                Ping,
                Stop,
            }

            loop {
                let next = tokio::select! {
                    event = registration.receiver.recv() => match event {
                        Some(event) => Next::Send(event),
                        None => Next::Stop,
                    },
                    frame = socket.recv_raw() => match frame {
                        Ok(Message::Close { .. }) | Err(_) => Next::Stop,
                        Ok(_) => {
                            last_seen = tokio::time::Instant::now();
                            continue;
                        }
                    },
                    _ = ping.tick() => {
                        if last_seen.elapsed() > liveness.timeout {
                            tracing::info!(host_id, "agent stream liveness timeout; closing");
                            Next::Stop
                        } else {
                            Next::Ping
                        }
                    }
                };
                match next {
                    Next::Send(event) => {
                        if socket.send(event).await.is_err() {
                            break;
                        }
                    }
                    Next::Ping => {
                        if socket.send_raw(Message::Ping(Bytes::new())).await.is_err() {
                            break;
                        }
                    }
                    Next::Stop => break,
                }
            }
        },
    ))
}

#[injectable::inject(host_repo: HostRepo, registry: Registry)]
#[post("/api/client/stream/disconnect", agent: AgentInfo)]
pub async fn disconnect() -> Result {
    let host = host_repo.get_by_mac(&agent.mac).await?;
    registry.deregister(host.id);
    Ok(())
}
