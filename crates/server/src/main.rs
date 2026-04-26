mod api;
mod domain;
mod error;
mod registry;
mod repository;

use std::{str::FromStr, sync::Arc};

use axum::Router;
use imaged_rpc::{client::v1::client_service_server, dashboard::v1::dashboard_service_server};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logging();

    let pool = setup_database().await?;
    let host_repo = repository::host_repo(pool);
    let host_registry = Arc::new(registry::HostRegistry::new());
    let client_service = client_service_server::ClientServiceServer::new(
        api::client::ClientHandler::new(host_repo.clone(), host_registry.clone()),
    );
    let dashboard_service = dashboard_service_server::DashboardServiceServer::new(
        api::dashboard::DashboardHandler::new(host_repo, host_registry),
    );

    let grpc_router = tonic::service::Routes::builder()
        .add_service(client_service)
        .add_service(dashboard_service)
        .clone()
        .routes()
        .into_axum_router()
        .layer(tonic_web::GrpcWebLayer::new());

    let routes = Router::new().merge(grpc_router);

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
