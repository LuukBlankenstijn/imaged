use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};

use crate::server::Server;
use crate::task::{self, ClientState};

#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub start: Duration,
    pub cap: Duration,
}

impl Backoff {
    pub const PRODUCTION: Backoff = Backoff {
        start: Duration::from_millis(250),
        cap: Duration::from_secs(5),
    };
}

pub async fn run(
    server: Arc<Server>,
    state: Arc<ClientState>,
    disk_size_bytes: u64,
    backoff: Backoff,
) -> ! {
    let mut delay = backoff.start;
    loop {
        match server.connect_events(disk_size_bytes).await {
            Ok(events) => {
                tracing::info!("agent control connection established");
                delay = backoff.start;
                drain(&server, &state, events).await;
                tracing::warn!("agent control connection closed, reconnecting");
            }
            Err(e) => {
                tracing::error!(err=%format_args!("{e:#}"), "failed to establish agent control connection");
            }
        }

        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(backoff.cap);
    }
}

async fn drain(
    server: &Arc<Server>,
    state: &Arc<ClientState>,
    events: impl Stream<Item = imaged_model::ServerEvent>,
) {
    let mut events = std::pin::pin!(events);
    while let Some(event) = events.next().await {
        task::handle_message(server, state.clone(), event).await;
    }
}
