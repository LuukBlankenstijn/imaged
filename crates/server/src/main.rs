mod api;
mod domain;
mod error;
mod multicast;
mod registry;
mod repository;
mod service;

use std::{net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc, time::Duration};

use clap::Parser;

use axum::{
    Router,
    serve::{Listener, ListenerExt},
};
use imaged_rpc::dashboard::v1::dashboard_service_server;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use crate::{api::dashboard::DashboardHandler, multicast::MulticastManager};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// address to bind to, also used as the wake on lan interface
    #[arg(short, long, default_value_t = SocketAddr::from(([0,0,0,0], 8080)))]
    bind_address: SocketAddr,
    #[arg(short, long, default_value = "info")]
    log_level: String,
    /// Address to bind the web routes to, if not set will be the same as bind_address
    #[arg(short, long)]
    web_bind_address: Option<SocketAddr>,
    /// network interface udp-sender binds for multicast deploys
    #[arg(short, long, env = "MULTICAST_INTERFACE", default_value = "lo")]
    multicast_interface: String,
    /// directory of built dashboard assets to serve; unset disables static serving
    #[arg(short, long, env = "ASSETS_DIR")]
    assets_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    imaged_shared::setup_logging!(args.log_level);

    let pool = setup_database().await?;
    let host_repo = repository::host_repo(pool.clone());
    let image_repo = repository::image_repo(pool.clone());
    let task_repo = repository::task_repo(pool.clone());
    let group_repo = repository::group_repo(pool);
    let host_registry = Arc::new(registry::HostRegistry::new());
    let image_service = Arc::new(service::image::ImageService::new("images".to_string()));
    let multicast_manager = Arc::new(
        MulticastManager::new(
            task_repo.clone(),
            image_repo.clone(),
            image_service.clone(),
            args.multicast_interface,
        )
        .await?,
    );

    let handler_state = Arc::new(api::HandlerState::new(
        host_repo,
        host_registry,
        image_repo,
        task_repo,
        group_repo,
        image_service,
        multicast_manager,
        args.bind_address,
    ));
    let dashboard_service = dashboard_service_server::DashboardServiceServer::new(
        DashboardHandler::new(handler_state.clone()),
    );

    let grpc_router = tonic::service::Routes::builder()
        .add_service(dashboard_service)
        .clone()
        .routes()
        .into_axum_router()
        .layer(tonic_web::GrpcWebLayer::new());

    let machine_router =
        api::pxe::router().merge(api::client::router().with_state(handler_state));
    let web_router = build_web_router(grpc_router, args.assets_dir);

    let main_listener = bind(args.bind_address).await?;
    match args.web_bind_address {
        Some(web_bind_address) => {
            let web_listener = bind(web_bind_address).await?;
            tracing::info!(
                bind_address = %args.bind_address,
                web_bind_address = %web_bind_address,
                "starting imaged-server"
            );
            tokio::try_join!(
                serve(main_listener, machine_router),
                serve(web_listener, web_router),
            )?;
        }
        None => {
            tracing::info!(bind_address = %args.bind_address, "starting imaged-server");
            serve(main_listener, machine_router.merge(web_router)).await?;
        }
    }

    Ok(())
}

const DEAD_CONNECTION_TIMEOUT: Duration = Duration::from_secs(20);

async fn bind(address: SocketAddr) -> std::io::Result<impl Listener<Addr = SocketAddr>> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    Ok(listener.tap_io(|stream| {
        if let Err(e) =
            socket2::SockRef::from(&*stream).set_tcp_user_timeout(Some(DEAD_CONNECTION_TIMEOUT))
        {
            tracing::warn!(error = %e, "failed to set connection death timeout");
        }
    }))
}

async fn serve(listener: impl Listener<Addr = SocketAddr>, router: Router) -> std::io::Result<()> {
    let service = router.layer(TraceLayer::new_for_http()).into_make_service();
    axum::serve(listener, service).await
}

fn build_web_router(grpc: Router, assets_dir: Option<PathBuf>) -> Router {
    let router = Router::new().nest("/api", grpc);
    match assets_dir {
        Some(dir) => router.fallback_service(spa_service(dir)),
        None => router,
    }
}

fn spa_service(dir: PathBuf) -> ServeDir<ServeFile> {
    let index_html = dir.join("index.html");
    ServeDir::new(dir).fallback(ServeFile::new(index_html))
}

async fn setup_database() -> Result<SqlitePool, Box<dyn std::error::Error>> {
    let sqlite_options = SqliteConnectOptions::from_str("sqlite://imaged.db")?
        .create_if_missing(true)
        .foreign_keys(true);
    let sqlite_pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(sqlite_options)
        .await?;

    sqlx::migrate!("./migrations").run(&sqlite_pool).await?;

    Ok(sqlite_pool)
}
