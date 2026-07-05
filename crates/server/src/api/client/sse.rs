use std::sync::Arc;

use super::HandlerState;
use crate::{
    api::client::AgentMac,
    error::{AppError, Result},
};
use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use futures::stream::Stream;
use imaged_shared::{ServerEvent, Task};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

#[derive(Deserialize)]
pub struct StartStreamQuery {
    pub disk_size_bytes: u64,
}

pub async fn start_stream(
    State(state): State<Arc<HandlerState>>,
    Query(req): Query<StartStreamQuery>,
    AgentMac(mac): AgentMac,
) -> Result<Sse<impl Stream<Item = Result<Event>>>> {
    let host = state
        .host_repo
        .upsert_host(mac, req.disk_size_bytes)
        .await?;

    let (tx, rx) = mpsc::channel::<Result<Event>>(8);
    let mut registration = state.host_registry.register(host.id)?;
    let task = state.task_repo.get_next(host.id).await?;
    if let Some(task) = task {
        let evt = ServerEvent::from(Task::new(task.id, task.task_type.into(), task.image_id));
        let sse_event = match Event::default().json_data(evt) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("failed to serialize sse event: {e}");
                return Err(AppError::Internal("serialize error".to_string()));
            }
        };
        // will never fail
        let _ = tx.send(Ok(sse_event)).await;
    }

    tokio::spawn(async move {
        loop {
            tokio::select! {
                maybe_event = registration.receiver.recv() => {
                    match maybe_event {
                        Some(event) => {
                            let sse_event = match Event::default().json_data(event) {
                                Ok(e) => e,
                                Err(e) => {
                                    tracing::error!("failed to serialize sse event: {e}");
                                    break;
                                }
                            };
                            if tx.send(Ok(sse_event)).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = tx.closed() => break,
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()))
}

pub async fn disconnect(
    State(state): State<Arc<HandlerState>>,
    AgentMac(mac): AgentMac,
) -> Result<()> {
    let host = state.host_repo.get_by_mac(&mac).await?;
    state.host_registry.deregister(host.id);
    Ok(())
}
