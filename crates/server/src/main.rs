#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(imaged_web::App);
}

#[cfg(feature = "server")]
mod config;

#[cfg(feature = "server")]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("imaged-server: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "server")]
async fn run() -> std::io::Result<()> {
    use clap::Parser;
    use imaged_server::Imaged;

    #[derive(Parser)]
    #[command(version, about)]
    struct Args {
        #[arg(short, long, env = "IMAGED_CONFIG")]
        config: std::path::PathBuf,
    }

    let args = Args::parse();
    let file = config::FileConfig::load(&args.config)?;
    imaged_model::setup_logging!(
        file.log_level,
        [
            "imaged_core",
            "imaged_boot",
            "imaged_proxy",
            "imaged_api_ui",
            "imaged_api_agent"
        ]
    );
    let (config, listeners) = file.into_server()?;
    let imaged = Imaged::open(config).await?;
    let server = imaged.serve(listeners).await?;

    tokio::select! {
        requested = stop_requested() => requested?,
        () = server.failed() => {}
    }
    server.stop().await
}

#[cfg(all(feature = "server", unix))]
async fn stop_requested() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        interrupted = tokio::signal::ctrl_c() => interrupted,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(all(feature = "server", not(unix)))]
async fn stop_requested() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}
