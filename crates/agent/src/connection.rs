use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};

use crate::agent::Agent;
use crate::status::Connection;
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
    agent: Agent,
    state: Arc<ClientState>,
    disk_size_bytes: u64,
    backoff: Backoff,
) -> ! {
    let mut delay = backoff.start;
    loop {
        agent
            .status
            .update(|s| s.connection = Connection::Connecting);
        let lost = match agent
            .server
            .connect_events(disk_size_bytes, agent.status.uplink())
            .await
        {
            Ok(events) => {
                tracing::info!("agent control connection established");
                agent
                    .status
                    .update(|s| s.connection = Connection::Connected);
                delay = backoff.start;
                drain(&agent, &state, events).await;
                tracing::warn!("agent control connection closed, reconnecting");
                "the server closed the connection".to_string()
            }
            Err(e) => {
                tracing::error!(err=%format_args!("{e:#}"), "failed to establish agent control connection");
                format!("{e:#}")
            }
        };
        agent
            .status
            .update(|s| s.connection = Connection::Lost(lost));

        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(backoff.cap);
    }
}

async fn drain(
    agent: &Agent,
    state: &Arc<ClientState>,
    events: impl Stream<Item = imaged_model::ServerEvent>,
) {
    let mut events = std::pin::pin!(events);
    while let Some(event) = events.next().await {
        task::handle_message(agent, state.clone(), event).await;
    }
}
