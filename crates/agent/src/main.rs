use imaged_agent::agent::Agent;
use imaged_agent::status::{Identity, Status};
use imaged_agent::{connection, logging, server::Server, shell, sys, task, ui};

use std::io::IsTerminal as _;
use std::sync::Arc;

use clap::Parser;
use reqwest::Url;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Base url of the imaged server to connect to
    #[arg(default_value_t = Url::parse("https://192.168.0.1:8080").expect("invalid url"))]
    server: Url,
    #[arg(short, long, default_value = "info")]
    log_level: String,
    /// Print plain logs instead of drawing the dashboard on the terminal
    #[arg(long)]
    no_tui: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let tui = !args.no_tui && std::io::stdout().is_terminal();
    let logs = logging::init(&args.log_level, tui);

    let mac = sys::get_mac()?;
    let ip = sys::get_ip();
    let disk = sys::disk::find_target_disk().await?;

    let server = Arc::new(Server::new(args.server.clone(), mac, ip)?);
    let status = Status::new(Identity {
        mac: mac.to_string().to_ascii_lowercase(),
        ip: ip.map(|ip| ip.to_string()),
        server: server.base_url().to_string(),
    });
    let agent = Agent { server, status };
    let client = Arc::new(task::ClientState::default());

    tracing::info!(
        server = %args.server,
        mac = %mac,
        ip = %ip.map(|ip| ip.to_string()).unwrap_or_else(|| "unknown".into()),
        "starting imaged-agent"
    );

    if tui {
        let (agent, client) = (agent.clone(), client.clone());
        tokio::spawn(async move {
            if let Err(e) = ui::run(agent, client, logs).await {
                tracing::error!(error=%e, "the dashboard stopped");
            }
        });
    } else {
        tokio::spawn(shell::watch_for_shell_hotkey());
    }

    connection::run(agent, client, disk.size, connection::Backoff::PRODUCTION).await
}
