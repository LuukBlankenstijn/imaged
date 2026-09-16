use imaged_client::{connection, shell, sys, task, transport};

use std::sync::Arc;

use clap::Parser;
use dioxus_fullstack::reqwest::Url;
use imaged_shared::setup_logging;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Base url of the imaged server to connect to
    #[arg(default_value_t = Url::parse("https://192.168.0.1:8080").expect("invalid url"))]
    server: Url,
    #[arg(short, long, default_value = "debug")]
    log_level: String,
}

// dioxus-fullstack's native client wraps every response body in a `SendWrapper`,
// which panics when polled from a thread other than the one that created it.
// A single-threaded runtime keeps every poll on the thread that issued the request.
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    setup_logging!(args.log_level, ["scuttlecast"]);

    let mac = sys::get_mac()?;
    let ip = sys::get_ip();
    let disk = sys::disk::find_target_disk().await?;

    transport::setup_transport(args.server.clone(), mac, ip)?;
    let state = Arc::new(task::ClientState::default());

    tracing::info!(
        server=%args.server.to_string(),
        mac=%mac.to_string(),
        ip=%ip.map(|ip| ip.to_string()).unwrap_or("unknown".to_string()),
        "starting imaged-client"
    );

    tokio::spawn(shell::watch_for_shell_hotkey());

    connection::run(state, disk.size, connection::Backoff::PRODUCTION).await
}
