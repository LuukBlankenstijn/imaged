mod sys;
mod transport;

use clap::Parser;
use futures::StreamExt;
use tracing_subscriber::EnvFilter;

use crate::transport::start_stream;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    server: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let server = args.server;
    let mac = sys::get_mac()?;
    let disk = sys::find_target_disk()?;

    let mut stream = start_stream(server, mac, disk.size).await?;

    while let Some(message) = stream.next().await {
        match message {
            Ok(message) => tracing::info!(msg=%message, "received message"),
            Err(e) => tracing::error!(err=%e, "received stream error"),
        }
    }

    tracing::info!("starting imaged-client");

    Ok(())
}
