use imaged_web::App;

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}

#[cfg(feature = "server")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::net::SocketAddr;

    use clap::Parser;
    use dioxus::server::{DioxusRouterExt, ServeConfig};
    use imaged_core as core;
    use imaged_core::di;

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
    }

    let args = Args::parse();
    imaged_shared::setup_logging!(args.log_level);

    let pool = core::setup_database("sqlite://imaged.db").await?;
    let container = core::build_di_container(
        pool,
        "images".to_string(),
        args.multicast_interface,
        args.bind_address,
    )
    .await?;

    di::init_container(container.clone());

    let machine_router = core::api::pxe::router()
        .merge(core::api::client::router().with_state(std::sync::Arc::new(container)));
    let web_router = axum::Router::new().serve_dioxus_application(ServeConfig::new(), App);

    let main_listener = core::bind(args.bind_address).await?;
    match args.web_bind_address {
        Some(web_bind_address) => {
            let web_listener = core::bind(web_bind_address).await?;
            tracing::info!(
                bind_address = %args.bind_address,
                web_bind_address = %web_bind_address,
                "starting imaged-server"
            );
            tokio::try_join!(
                core::serve(main_listener, machine_router),
                core::serve(web_listener, web_router),
            )?;
        }
        None => {
            tracing::info!(bind_address = %args.bind_address, "starting imaged-server");
            core::serve(main_listener, machine_router.merge(web_router)).await?;
        }
    }

    Ok(())
}
