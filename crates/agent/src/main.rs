use imaged_agent::{connection, server::Server, shell, sys, task};

use std::sync::Arc;

use clap::Parser;
use imaged_model::setup_logging;
use reqwest::Url;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Base url of the imaged server to connect to
    #[arg(default_value_t = Url::parse("https://192.168.0.1:8080").expect("invalid url"))]
    server: Url,
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    setup_logging!(args.log_level, ["scuttlecast"]);

    let mac = sys::get_mac()?;
    let ip = sys::get_ip();
    let disk = sys::disk::find_target_disk().await?;

    let server = Arc::new(Server::new(args.server.clone(), mac, ip)?);
    let state = Arc::new(task::ClientState::default());

    tracing::info!(
        server = %args.server,
        mac = %mac,
        ip = %ip.map(|ip| ip.to_string()).unwrap_or_else(|| "unknown".into()),
        "starting imaged-agent"
    );

    tokio::spawn(shell::watch_for_shell_hotkey());

    connection::run(server, state, disk.size, connection::Backoff::PRODUCTION).await
}
