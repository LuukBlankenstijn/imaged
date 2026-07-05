mod shell;
mod sys;
mod task;
mod transport;

use std::sync::Arc;

use clap::Parser;
use futures::StreamExt;
use futures::pin_mut;
use imaged_shared::setup_logging;
use reqwest::Url;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Base url of the imaged server to connect to
    #[arg(default_value_t = Url::parse("https://192.168.0.1:8080").expect("invalid url"))]
    server: Url,
    #[arg(short, long, default_value = "debug")]
    log_level: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    setup_logging!(args.log_level);

    let mac = sys::get_mac()?;
    let ip = sys::get_ip();
    let disk = sys::disk::find_target_disk().await?;

    // create the state
    let state = Arc::new(task::ClientState::new(args.server.clone(), mac, ip)?);
    // start the stream
    tracing::info!(
        server=%args.server.to_string(),
        mac=%mac.to_string(),
        ip=%ip.map(|ip| ip.to_string()).unwrap_or("unknown".to_string()),
        "starting stream"
    );
    let stream = state.client().start_stream(disk.size).await?;
    pin_mut!(stream);

    // start the handler for the shell
    tokio::spawn(shell::watch_for_shell_hotkey());
    tracing::info!("starting imaged-client");
    while let Some(message) = stream.next().await {
        match message {
            Ok(message) => task::handle_message(state.clone(), message.event).await,
            Err(e) => tracing::error!(err=%e, "received stream error"),
        }
    }

    Ok(())
}
