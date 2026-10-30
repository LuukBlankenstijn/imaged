pub mod di;
pub mod domain;
pub mod listener;
pub mod multicast;
pub mod progress;
pub mod registry;
pub mod repository;
pub mod service;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

use std::{
    collections::BTreeMap, future::Future, net::SocketAddr, path::Path, sync::Arc, time::Duration,
};

use axum::{Router, serve::Listener};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
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
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
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
    script_variables: BTreeMap<String, String>,
) -> Result<DIContainer, BoxError> {
    for (name, value) in &script_variables {
        imaged_model::script_env::check_variable(name, value)?;
    }
    let host_repo = repository::host_repo(pool.clone());
    let image_repo = repository::image_repo(pool.clone());
    let task_repo = repository::task_repo(pool.clone());
    let script_repo = repository::script_repo(pool.clone());
    let group_repo = repository::group_repo(pool);
    let host_registry = Arc::new(registry::HostRegistry::default());
    let progress_board = Arc::new(progress::ProgressBoard::default());
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
            progress_board.clone(),
            multicast_interface,
        )
        .await?,
    );
    let machine_ip = Arc::default();
    let agent_liveness = AgentLiveness {
        ping_interval: Duration::from_secs(1),
        timeout: Duration::from_secs(30),
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
        progress_board,
        machine_ip,
        agent_liveness,
        script_proxy.map(Arc::from),
        Arc::new(script_variables),
    ))
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

#[cfg(test)]
mod tests {
    use sqlx::{Row, migrate::Migrator};

    use super::*;
    use crate::test_support::TestDir;

    const MAC_CASE_MIGRATION: i64 = 20261021130000;

    async fn database_before(dir: &TestDir, version: i64) -> SqlitePool {
        let older = dir.path().join("older-migrations");
        std::fs::create_dir_all(&older).unwrap();
        let all = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        for entry in std::fs::read_dir(all).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let migration_version: i64 = name.split('_').next().unwrap().parse().unwrap();
            if migration_version < version {
                std::fs::copy(&path, older.join(&name)).unwrap();
            }
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(dir.path().join("upgrade.db"))
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        Migrator::new(older)
            .await
            .unwrap()
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    async fn count(pool: &SqlitePool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn canonicalising_macs_merges_case_twins_without_losing_task_history() {
        let dir = TestDir::new();
        let pool = database_before(&dir, MAC_CASE_MIGRATION).await;
        sqlx::raw_sql(
            "INSERT INTO hosts (id, mac, name, disk_size_bytes) VALUES
               (1, 'aa:bb:cc:dd:ee:01', 'aa-bb-cc-dd-ee-01', 1),
               (2, 'AA:BB:CC:DD:EE:01', 'AA-BB-CC-DD-EE-01', 1),
               (3, 'AA:BB:CC:DD:EE:03', 'AA-BB-CC-DD-EE-03', 1),
               (4, 'AA-BB-CC-DD-EE-03', 'bench', 1),
               (5, 'aa:bb:cc:dd:ee:05', 'lab', 1);
             INSERT INTO groups (id, name) VALUES (1, 'rack');
             INSERT INTO group_hosts (group_id, host_id) VALUES (1, 2), (1, 4);
             INSERT INTO tasks (id, type, created_at) VALUES
               (1, 'deploy', '2026-01-01T00:00:00Z'),
               (2, 'reboot', '2026-01-01T00:00:00Z'),
               (3, 'deploy', '2026-01-01T00:00:00Z');
             INSERT INTO task_hosts (task_id, host_id, state) VALUES
               (1, 2, 'done'), (2, 3, 'done'), (3, 4, 'running');
             INSERT INTO task_host_inputs (task_id, host_id, name, value) VALUES
               (1, 2, 'hostname', 'pc-1'), (3, 4, 'hostname', 'bench-1');
             INSERT INTO script_runs (task_id, host_id, position, script_name, started_at, output_bytes)
               VALUES (3, 4, 0, 'setup', '2026-01-01T00:00:00Z', 3);
             INSERT INTO script_run_output (task_id, host_id, position, start, text)
               VALUES (3, 4, 0, 0, 'ok\n');",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let hosts: Vec<(i64, String, String)> =
            sqlx::query("SELECT id, mac, name FROM hosts ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap()
                .iter()
                .map(|row| (row.get(0), row.get(1), row.get(2)))
                .collect();
        assert_eq!(
            hosts,
            [
                (2, "aa:bb:cc:dd:ee:01".into(), "aa-bb-cc-dd-ee-01".into()),
                (3, "aa:bb:cc:dd:ee:03".into(), "aa-bb-cc-dd-ee-03".into()),
                (5, "aa:bb:cc:dd:ee:05".into(), "lab".into()),
            ]
        );
        let task_hosts: Vec<(i64, i64, String)> =
            sqlx::query("SELECT task_id, host_id, state FROM task_hosts ORDER BY task_id")
                .fetch_all(&pool)
                .await
                .unwrap()
                .iter()
                .map(|row| (row.get(0), row.get(1), row.get(2)))
                .collect();
        assert_eq!(
            task_hosts,
            [
                (1, 2, "done".into()),
                (2, 3, "done".into()),
                (3, 3, "running".into())
            ]
        );
        let inputs: Vec<(i64, i64, String)> =
            sqlx::query("SELECT task_id, host_id, value FROM task_host_inputs ORDER BY task_id")
                .fetch_all(&pool)
                .await
                .unwrap()
                .iter()
                .map(|row| (row.get(0), row.get(1), row.get(2)))
                .collect();
        assert_eq!(inputs, [(1, 2, "pc-1".into()), (3, 3, "bench-1".into())]);
        let run: (String, i64, String) = sqlx::query(
            "SELECT r.script_name, r.output_bytes, o.text FROM script_runs r \
             JOIN script_run_output o USING (task_id, host_id, position) \
             WHERE r.task_id = 3 AND r.host_id = 3",
        )
        .map(|row: sqlx::sqlite::SqliteRow| (row.get(0), row.get(1), row.get(2)))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(run, ("setup".into(), 3, "ok\n".into()));
        assert_eq!(
            count(
                &pool,
                "SELECT count(*) FROM group_hosts WHERE host_id IN (2, 3)"
            )
            .await,
            2
        );
        assert_eq!(
            count(&pool, "SELECT count(*) FROM pragma_foreign_key_check").await,
            0
        );
    }
}
