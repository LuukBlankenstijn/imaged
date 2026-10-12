pub mod di;
pub mod domain;
pub mod error;
pub mod listener;
pub mod multicast;
pub mod pxe;
pub mod registry;
pub mod repository;
pub mod service;
pub mod tftp;

use std::{future::Future, net::SocketAddr, path::Path, sync::Arc, time::Duration};

use axum::{Router, serve::Listener};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tower_http::trace::TraceLayer;

use crate::di::DIContainer;
use crate::multicast::MulticastManager;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub const DEAD_CONNECTION_TIMEOUT: Duration = Duration::from_secs(20);

/// Cadence and deadline for the agent control-connection liveness protocol.
/// The timeout is a multiple of the interval so a single missed pong cannot
/// evict a healthy agent; eviction requires two consecutive missed pings.
#[derive(Clone, Copy)]
pub struct AgentLiveness {
    pub ping_interval: Duration,
    pub timeout: Duration,
}

pub async fn setup_database(path: &Path) -> Result<SqlitePool, BoxError> {
    let sqlite_options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
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
    script_proxy: Option<String>,
) -> Result<DIContainer, BoxError> {
    let host_repo = repository::host_repo(pool.clone());
    let image_repo = repository::image_repo(pool.clone());
    let task_repo = repository::task_repo(pool.clone());
    let script_repo = repository::script_repo(pool.clone());
    let group_repo = repository::group_repo(pool);
    let host_registry = Arc::new(registry::HostRegistry::default());
    let image_service = Arc::new(service::image::ImageService::new(images_dir));
    let archive_service = Arc::new(
        service::archive::ArchiveService::start(
            image_repo.clone(),
            script_repo.clone(),
            image_service.clone(),
        )
        .await?,
    );
    let multicast_manager = Arc::new(
        MulticastManager::start(
            task_repo.clone(),
            host_repo.clone(),
            image_repo.clone(),
            image_service.clone(),
            host_registry.clone(),
            multicast_interface,
        )
        .await?,
    );
    let machine_ip = Arc::default();
    let agent_liveness = AgentLiveness {
        ping_interval: Duration::from_secs(1),
        timeout: Duration::from_secs(3),
    };

    Ok(DIContainer::new(
        host_repo,
        image_repo,
        task_repo,
        group_repo,
        script_repo,
        host_registry,
        image_service,
        archive_service,
        multicast_manager,
        machine_ip,
        agent_liveness,
        script_proxy.map(Arc::from),
    ))
}

#[cfg(any(test, feature = "test-support"))]
pub async fn build_test_container(dir: &Path) -> DIContainer {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let pool = setup_database(&dir.join("test.db")).await.unwrap();
    let mut container = build_di_container(
        pool,
        dir.join("images").to_string_lossy().to_string(),
        "lo".to_string(),
        None,
    )
    .await
    .unwrap();
    container.agent_liveness = AgentLiveness {
        ping_interval: Duration::from_millis(200),
        timeout: Duration::from_millis(600),
    };
    container
}

pub async fn serve(
    listener: impl Listener<Addr = SocketAddr>,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let service = router.layer(TraceLayer::new_for_http()).into_make_service();
    axum::serve(listener, service)
        .with_graceful_shutdown(shutdown)
        .await
}
