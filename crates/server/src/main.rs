#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(imaged_web::App);
}

#[cfg(feature = "server")]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    use std::{net::SocketAddr, path::PathBuf};

    use clap::{Parser, builder::FalseyValueParser};
    use imaged_server::{Config, Dashboard, Imaged, Listeners};

    #[derive(Parser)]
    #[command(version, about)]
    struct Args {
        #[arg(short, long, default_value_t = dioxus::cli_config::fullstack_address_or_localhost())]
        bind_address: SocketAddr,
        #[arg(short, long, default_value = "info")]
        log_level: String,
        #[arg(short, long)]
        web_bind_address: Option<SocketAddr>,
        #[arg(short, long, env = "MULTICAST_INTERFACE", default_value = "lo")]
        multicast_interface: String,
        #[arg(long)]
        tftp_bind_address: Option<SocketAddr>,
        #[arg(long, default_value = ".")]
        data_dir: PathBuf,
        #[arg(long, env = "PUBLIC_BASE", default_value = "localhost:8080")]
        public_base: String,
        #[arg(long, env = "USE_HTTPS", value_parser = FalseyValueParser::new())]
        use_https: bool,
        #[arg(long, env = "SCRIPT_PROXY")]
        script_proxy: Option<String>,
    }

    let args = Args::parse();
    imaged_shared::setup_logging!(args.log_level, ["imaged_core"]);

    let scheme = if args.use_https { "https" } else { "http" };
    let imaged = Imaged::open(Config {
        data_dir: args.data_dir,
        multicast_interface: args.multicast_interface,
        script_proxy: args.script_proxy,
    })
    .await?;
    let server = imaged
        .serve(Listeners {
            machine: args.bind_address,
            public_url: format!("{scheme}://{}", args.public_base),
            tftp: args.tftp_bind_address,
            dashboard: args
                .web_bind_address
                .map_or(Dashboard::OnMachineListener, Dashboard::Separate),
        })
        .await?;

    tokio::select! {
        interrupted = tokio::signal::ctrl_c() => interrupted?,
        () = server.failed() => {}
    }
    server.stop().await?;
    Ok(())
}
