use std::sync::Arc;
use std::time::Duration;

use dioxus_fullstack::WebSocketOptions;

use crate::task::{self, ClientState};

#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub start: Duration,
    pub cap: Duration,
}

impl Backoff {
    pub const PRODUCTION: Backoff = Backoff {
        start: Duration::from_secs(1),
        cap: Duration::from_secs(30),
    };
}

pub async fn run(state: Arc<ClientState>, disk_size_bytes: u64, backoff: Backoff) -> ! {
    let mut delay = backoff.start;
    loop {
        match api::event::start_stream(disk_size_bytes, WebSocketOptions::new()).await {
            Ok(socket) => {
                tracing::info!("agent control connection established");
                delay = backoff.start;
                drain(&state, socket).await;
                tracing::warn!("agent control connection closed, reconnecting");
            }
            Err(e) => {
                tracing::error!(err=%e, "failed to establish agent control connection");
            }
        }

        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(backoff.cap);
    }
}

async fn drain(
    state: &Arc<ClientState>,
    socket: dioxus_fullstack::Websocket<(), imaged_shared::ServerEvent>,
) {
    loop {
        match socket.recv().await {
            Ok(event) => task::handle_message(state.clone(), event).await,
            Err(e) => {
                tracing::info!(err=%e, "agent control stream ended");
                break;
            }
        }
    }
}
