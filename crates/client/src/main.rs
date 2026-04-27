mod error;
mod sys;
mod task;
mod transport;

use std::sync::Arc;

use clap::Parser;
use futures::StreamExt;
use futures::pin_mut;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::transport::ApiClient;
use crate::transport::sse::start_stream;

struct ClientState {
    http: ApiClient,
    current_task: Mutex<Option<RunningTask>>,
}

struct RunningTask {
    task_id: i64,
    // can be used for forcefully aborting the task
    _handle: JoinHandle<()>,
    cancel: tokio_util::sync::CancellationToken,
}

#[derive(Parser)]
#[command(version, about)]
struct Args {
    server: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    imaged_shared::setup_logging();

    let args = Args::parse();
    let server = args.server;
    let mac = sys::mac::get_mac()?;
    let disk = sys::disk::find_target_disk().await?;

    let stream = start_stream(server.clone(), mac.clone(), disk.size).await?;
    pin_mut!(stream);

    let state = Arc::new(ClientState {
        http: ApiClient::new(server, mac)?,
        current_task: Mutex::new(None),
    });

    tracing::info!("starting imaged-client");
    while let Some(message) = stream.next().await {
        match message {
            Ok(message) => task::handle_message(state.clone(), message.event).await,
            Err(e) => tracing::error!(err=%e, "received stream error"),
        }
    }

    Ok(())
}
