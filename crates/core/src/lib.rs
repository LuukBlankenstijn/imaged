pub mod di;
pub mod domain;
pub mod error;
pub mod multicast;
pub mod pxe;
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

use crate::di::DIContainer;
use crate::multicast::MulticastManager;

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

pub async fn build_di_container(
    pool: SqlitePool,
    images_dir: String,
    multicast_interface: String,
    bind_address: SocketAddr,
) -> Result<DIContainer, Box<dyn std::error::Error>> {
    let host_repo = repository::host_repo(pool.clone());
    let image_repo = repository::image_repo(pool.clone());
    let task_repo = repository::task_repo(pool.clone());
    let group_repo = repository::group_repo(pool);
    let host_registry = Arc::new(registry::HostRegistry::default());
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

    Ok(DIContainer::new(
        host_repo,
        image_repo,
        task_repo,
        group_repo,
        host_registry,
        image_service,
        multicast_manager,
        bind_address,
    ))
}

#[cfg(any(test, feature = "test-support"))]
pub async fn build_test_container(dir: &std::path::Path) -> DIContainer {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let pool = setup_database(&format!("sqlite://{}", dir.join("test.db").display()))
        .await
        .unwrap();
    build_di_container(
        pool,
        dir.join("images").to_string_lossy().to_string(),
        "lo".to_string(),
        "127.0.0.1:8080".parse().unwrap(),
    )
    .await
    .unwrap()
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
