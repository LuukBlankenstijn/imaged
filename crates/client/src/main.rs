mod error;
mod shell;
mod sys;
mod task;
mod transport;

use std::sync::Arc;

use clap::Parser;
use futures::StreamExt;
use futures::pin_mut;
use imaged_shared::setup_logging;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    server: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logging!("debug");

    let args = Args::parse();
    let server = args.server;
    let mac = sys::mac::get_mac()?;
    let disk = sys::disk::find_target_disk().await?;

    // create the state
    let state = Arc::new(task::ClientState::new(server, mac)?);
    // start the stream
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
