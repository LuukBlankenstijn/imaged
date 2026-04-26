mod api;
mod domain;
mod error;
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
use tracing_subscriber::EnvFilter;

use crate::api::dashboard::DashboardHandler;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logging();

    let pool = setup_database().await?;
    let host_repo = repository::host_repo(pool.clone());
    let host_registry = Arc::new(registry::HostRegistry::new());
    let image_repo = repository::image_repo(pool);
    let image_service = Arc::new(service::image::ImageService::new("images".to_string()));

    let handler_state = Arc::new(api::HandlerState::new(
        host_repo.clone(),
        host_registry.clone(),
        image_repo.clone(),
        image_service.clone(),
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
        .merge(api::client::router().with_state(handler_state))
        .merge(grpc_router);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    let service = routes.into_make_service();
    tracing::info!(interface = "0.0.0.0", port = 8080, "starting imaged-server");
    axum::serve(listener, service).await?;
    Ok(())
}

fn setup_logging() {
    let target_level = "info";
    let pkg_name = env!("CARGO_PKG_NAME");

    let filter_str = format!("{},{}=debug", target_level, pkg_name).replace("-", "_");

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(filter_str.clone())),
        )
        .init();

    tracing::info!(filter_str);
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
