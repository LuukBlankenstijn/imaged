use imaged_client::{shell, sys, task, transport};

use std::sync::Arc;

use clap::Parser;
use dioxus_fullstack::reqwest::Url;
use futures::StreamExt;
use futures::pin_mut;
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
    setup_logging!(args.log_level);

    let mac = sys::get_mac()?;
    let ip = sys::get_ip();
    let disk = sys::disk::find_target_disk().await?;

    transport::setup_transport(args.server.clone(), mac, ip)?;
    // create the state
    let state = Arc::new(task::ClientState::default());
    // start the stream
    tracing::info!(
        server=%args.server.to_string(),
        mac=%mac.to_string(),
        ip=%ip.map(|ip| ip.to_string()).unwrap_or("unknown".to_string()),
        "starting stream"
    );
    let stream = api::event::start_stream(disk.size).await?;
    pin_mut!(stream);

    // start the handler for the shell
    tokio::spawn(shell::watch_for_shell_hotkey());
    tracing::info!("starting imaged-client");
    while let Some(message) = stream.next().await {
        match message {
            Ok(message) => task::handle_message(state.clone(), message).await,
            Err(e) => tracing::error!(err=%e, "received stream error"),
        }
    }

    Ok(())
}
