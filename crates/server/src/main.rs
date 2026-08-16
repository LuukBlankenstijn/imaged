use imaged_web::App;

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}

#[cfg(feature = "server")]
const CLIENT_API_PREFIX: &str = "/api/client";

/// Routes the subset of registered server functions whose path passes `keep`,
/// so the agent API and the dashboard API can be bound to different ports.
#[cfg(feature = "server")]
fn server_fns(keep: impl Fn(&str) -> bool) -> axum::Router<dioxus::server::FullstackState> {
    dioxus::server::ServerFunction::collect()
        .into_iter()
        .filter(|f| keep(f.path()))
        .fold(axum::Router::new(), |router, f| {
            router.route(f.path(), f.method_router())
        })
}

#[cfg(feature = "server")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::net::SocketAddr;

    use clap::Parser;
    use dioxus::server::{DioxusRouterExt, ServeConfig};
    use imaged_core as core;
    use imaged_core::di;

    // Server functions self-register via `inventory` at load time, which only
    // happens for crates actually linked into the binary.
    use imaged_api_client as _;
    use imaged_api_ui as _;

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

    di::init_container(container);

    let machine_router = core::pxe::router().merge(
        server_fns(|path| path.starts_with(CLIENT_API_PREFIX))
            .with_state(dioxus::server::FullstackState::headless()),
    );

    let web_router = server_fns(|path| !path.starts_with(CLIENT_API_PREFIX))
        .serve_static_assets()
        .fallback(axum::routing::get(
            dioxus::server::FullstackState::render_handler,
        ))
        .with_state(dioxus::server::FullstackState::new(ServeConfig::new(), App));

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
