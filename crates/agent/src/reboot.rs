use std::time::Duration;

use anyhow::Result;
use tokio::time::timeout;
use tracing::{info, warn};

use crate::agent::Agent;
use crate::task::ClientState;

pub const COUNTDOWN: Duration = Duration::from_secs(5);
const REPORT_TIMEOUT: Duration = Duration::from_secs(5);
const REBOOTED_FROM_CONSOLE: &str = "Rebooted from the console";

pub async fn after_countdown(agent: &Agent) -> Result<()> {
    if !agent.status.reboot_countdown(COUNTDOWN).await {
        info!("reboot cancelled from the console");
        return Ok(());
    }
    now(agent).await
}

pub async fn from_console(agent: &Agent, client: &ClientState) -> Result<()> {
    let running = agent.status.snapshot().running_task().map(|task| task.id);
    if let Some(task_id) = running {
        client.cancel(task_id).await;
        let reported = timeout(
            REPORT_TIMEOUT,
            agent.server.mark_failed(task_id, REBOOTED_FROM_CONSOLE),
        )
        .await;
        if !matches!(reported, Ok(Ok(()))) {
            warn!(
                task_id,
                "could not report the interrupted task before rebooting"
            );
        }
    }
    now(agent).await
}

async fn now(agent: &Agent) -> Result<()> {
    if !matches!(
        timeout(REPORT_TIMEOUT, agent.server.disconnect()).await,
        Ok(Ok(()))
    ) {
        warn!("could not tell the server this machine is rebooting");
    }
    crate::sys::reboot()
}
