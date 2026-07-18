
pub mod api;
pub mod di;
pub mod domain;
pub mod error;
pub mod multicast;
pub mod registry;
pub mod repository;
pub mod service;

use std::{net::SocketAddr, str::FromStr, sync::Arc, time::Duration};

use axum::{
    Router,
    serve::{Listener, ListenerExt},
};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tower_http::trace::TraceLayer;

use crate::{api::HandlerState, multicast::MulticastManager};

pub const DEAD_CONNECTION_TIMEOUT: Duration = Duration::from_secs(20);

pub async fn setup_database(db_url: &str) -> Result<SqlitePool, Box<dyn std::error::Error>> {
    let sqlite_options = SqliteConnectOptions::from_str(db_url)?
        .create_if_missing(true)
        .foreign_keys(true);
    let sqlite_pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(sqlite_options)
        .await?;

    sqlx::migrate!("./migrations").run(&sqlite_pool).await?;

    Ok(sqlite_pool)
}

pub async fn build_handler_state(
    pool: SqlitePool,
    images_dir: String,
    multicast_interface: String,
    bind_address: SocketAddr,
) -> Result<Arc<HandlerState>, Box<dyn std::error::Error>> {
    let host_repo = repository::host_repo(pool.clone());
    let image_repo = repository::image_repo(pool.clone());
    let task_repo = repository::task_repo(pool.clone());
    let group_repo = repository::group_repo(pool);
    let host_registry = Arc::new(registry::HostRegistry::new());
    let image_service = Arc::new(service::image::ImageService::new(images_dir));
    let multicast_manager = Arc::new(
        MulticastManager::new(
            task_repo.clone(),
            image_repo.clone(),
            image_service.clone(),
            multicast_interface,
        )
        .await?,
    );

    Ok(Arc::new(HandlerState::new(
        host_repo,
        host_registry,
        image_repo,
        task_repo,
        group_repo,
        image_service,
        multicast_manager,
        bind_address,
    )))
}

pub async fn bind(address: SocketAddr) -> std::io::Result<impl Listener<Addr = SocketAddr>> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    Ok(listener.tap_io(|stream| {
        if let Err(e) =
            socket2::SockRef::from(&*stream).set_tcp_user_timeout(Some(DEAD_CONNECTION_TIMEOUT))
        {
            tracing::warn!(error = %e, "failed to set connection death timeout");
        }
    }))
}

pub async fn serve(
    listener: impl Listener<Addr = SocketAddr>,
    router: Router,
) -> std::io::Result<()> {
    let service = router.layer(TraceLayer::new_for_http()).into_make_service();
    axum::serve(listener, service).await
}
