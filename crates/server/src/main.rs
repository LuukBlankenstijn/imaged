mod api;
mod domain;
mod error;
mod multicast;
mod registry;
mod repository;
mod service;

use std::{str::FromStr, sync::Arc};

use axum::Router;
use imaged_rpc::dashboard::v1::dashboard_service_server;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tower_http::trace::TraceLayer;

use crate::{api::dashboard::DashboardHandler, multicast::MulticastManager};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    imaged_shared::setup_logging!("debug");

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

    let http = async {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
        let service = routes.into_make_service();
        tracing::info!(interface = "0.0.0.0", port = 8080, "starting imaged-server");
        axum::serve(listener, service).await?;
        Ok(())
    };

    let tftp = async { api::tftp::serve().await };
    tokio::try_join!(http, tftp)?;
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
