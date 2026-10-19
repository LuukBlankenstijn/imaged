#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(imaged_web::App);
}

#[cfg(feature = "server")]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

    use clap::Parser;
    use imaged_server::{Config, Dashboard, Imaged, Listeners};
    use serde::Deserialize;
    use toml_file_refs::ConfigLoader;

    #[derive(Parser)]
    #[command(version, about)]
    struct Args {
        #[arg(short, long, env = "IMAGED_CONFIG")]
        config: Option<PathBuf>,
    }

    #[derive(Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct FileConfig {
        log_level: String,
        data_dir: PathBuf,
        multicast_interface: String,
        public_url: String,
        script_proxy: Option<String>,
        listen: Listen,
        script_variables: BTreeMap<String, String>,
    }

    #[derive(Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct Listen {
        machine: SocketAddr,
        dashboard: Option<SocketAddr>,
        tftp: Option<SocketAddr>,
        proxy: Option<SocketAddr>,
    }

    impl Default for FileConfig {
        fn default() -> Self {
            Self {
                log_level: "info".to_string(),
                data_dir: PathBuf::from("."),
                multicast_interface: "lo".to_string(),
                public_url: "http://localhost:8080".to_string(),
                script_proxy: None,
                listen: Listen::default(),
                script_variables: BTreeMap::new(),
            }
        }
    }

    impl Default for Listen {
        fn default() -> Self {
            Self {
                machine: dioxus::cli_config::fullstack_address_or_localhost(),
                dashboard: None,
                tftp: None,
                proxy: None,
            }
        }
    }

    let args = Args::parse();
    let config = match &args.config {
        Some(path) => ConfigLoader::new()
            .load::<FileConfig>(path)
            .map_err(std::io::Error::other)?,
        None => FileConfig::default(),
    };
    imaged_model::setup_logging!(
        config.log_level,
        ["imaged_core", "imaged_boot", "imaged_proxy"]
    );

    let script_proxy = match (config.script_proxy, config.listen.proxy) {
        (Some(external), _) => Some(external),
        (None, Some(bind)) if bind.ip().is_unspecified() => {
            let public_url: axum::http::Uri =
                config.public_url.parse().map_err(std::io::Error::other)?;
            let host = public_url
                .host()
                .ok_or_else(|| std::io::Error::other("public_url has no host"))?;
            Some(format!("http://{host}:{}", bind.port()))
        }
        (None, Some(bind)) => Some(format!("http://{bind}")),
        (None, None) => None,
    };
    let imaged = Imaged::open(Config {
        data_dir: config.data_dir,
        multicast_interface: config.multicast_interface,
        script_proxy,
        script_variables: config.script_variables,
    })
    .await?;
    let server = imaged
        .serve(Listeners {
            machine: config.listen.machine,
            public_url: config.public_url,
            tftp: config.listen.tftp,
            proxy: config.listen.proxy,
            dashboard: config
                .listen
                .dashboard
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
