use std::sync::Arc;

use super::HandlerState;
use crate::{api::client::AgentMac, error::Result};
use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use futures::stream::Stream;
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

    tokio::spawn(async move {
        loop {
            tokio::select! {
                maybe_event = registration.receiver.recv() => {
                    match maybe_event {
                        Some(_event) => {
                            // let server_event = ServerEvent::NewTask { task_id: 0 };
                            // let sse_event = match Event::default().json_data(&server_event) {
                            //     Ok(e) => e,
                            //     Err(e) => {
                            //         tracing::error!("failed to serialize sse event: {e}");
                            //         break;
                            //     }
                            // };
                            let sse_event = Event::default();
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
