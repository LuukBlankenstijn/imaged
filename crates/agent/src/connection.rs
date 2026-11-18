use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use imaged_model::ServerEvent;
use imaged_model::command::CommandReport;
use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::command;
use crate::server::Uplink;
use crate::status::Connection;
use crate::sys::disk::HostDisk;
use crate::task::{self, ClientState};

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub retry_start: Duration,
    pub retry_cap: Duration,
    pub server_silence: Duration,
}

impl Timing {
    pub const PRODUCTION: Timing = Timing {
        retry_start: Duration::from_millis(250),
        retry_cap: Duration::from_secs(5),
        server_silence: Duration::from_secs(15),
    };
}

pub async fn run(
    agent: Agent,
    state: Arc<ClientState>,
    probe: impl AsyncFn() -> Result<HostDisk, String>,
    timing: Timing,
) -> ! {
    let mut delay = timing.retry_start;
    let mut last_disk = None;
    loop {
        agent
            .status
            .update(|s| s.connection = Connection::Connecting);
        let busy = state.current_task.lock().await.is_some();
        let disk = match last_disk.take().filter(|_| busy) {
            Some(known) => last_disk.insert(known),
            None => last_disk.insert(probe().await),
        };
        if let Err(reason) = disk {
            tracing::warn!(%reason, "no disk to image; the server refuses captures and deploys for this machine");
        }
        let (reports, commands) = mpsc::unbounded_channel();
        let uplink = Uplink {
            progress: agent.status.uplink(),
            idle: state.idle(),
            commands,
        };
        let lost = match agent
            .server
            .connect_events(disk, uplink, timing.server_silence)
            .await
        {
            Ok(events) => {
                tracing::info!("agent control connection established");
                agent
                    .status
                    .update(|s| s.connection = Connection::Connected);
                delay = timing.retry_start;
                drain(&agent, &state, events, reports).await;
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
        delay = (delay * 2).min(timing.retry_cap);
    }
}

async fn drain(
    agent: &Agent,
    state: &Arc<ClientState>,
    events: impl Stream<Item = ServerEvent>,
    reports: mpsc::UnboundedSender<CommandReport>,
) {
    let mut events = std::pin::pin!(events);
    while let Some(event) = events.next().await {
        match event {
            ServerEvent::Current(current) => {
                task::handle_current(agent, state.clone(), current).await
            }
            ServerEvent::Run(request) => {
                command::spawn(request, agent.status.snapshot().identity, reports.clone())
            }
        }
    }
}
