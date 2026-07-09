mod api;
mod domain;
mod error;
mod multicast;
mod registry;
mod repository;
mod service;

use std::{net::SocketAddr, str::FromStr, sync::Arc, time::Duration};

use clap::Parser;

use axum::{Router, serve::ListenerExt};
use imaged_rpc::dashboard::v1::dashboard_service_server;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tower_http::trace::TraceLayer;

use crate::{api::dashboard::DashboardHandler, multicast::MulticastManager};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// address to bind to, also used as the wake on lan interface
    #[arg(short, long, default_value_t = SocketAddr::from(([0,0,0,0], 8080)))]
    bind_address: SocketAddr,
    #[arg(short, long, default_value = "info")]
    log_level: String,
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
        MulticastManager::new(task_repo.clone(), image_repo.clone(), image_service.clone()).await?,
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

    let routes = Router::new()
        .merge(api::pxe::router())
        .merge(api::client::router().with_state(handler_state))
        .merge(grpc_router)
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(args.bind_address.to_owned())
        .await?
        .tap_io(|stream| {
            // Cap how long unacknowledged data may linger before the kernel
            // declares the connection dead.
            if let Err(e) =
                socket2::SockRef::from(&*stream).set_tcp_user_timeout(Some(Duration::from_secs(20)))
            {
                tracing::warn!("failed to set TCP_USER_TIMEOUT on connection: {e}");
            }
        });
    let service = routes.into_make_service();
    tracing::info!(bind_address=%&args.bind_address, "starting imaged-server");
    axum::serve(listener, service).await?;

    Ok(())
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
