use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use sqlx::SqlitePool;

use crate::{
    AgentLiveness, build_di_container,
    di::DIContainer,
    domain::{
        group::GroupRepository, host::HostRepository, image::ImageRepository,
        script::ScriptRepository, task::TaskRepository,
    },
    repository, setup_database,
};

static DIR_ID: AtomicU64 = AtomicU64::new(0);

pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new() -> Self {
        let id = DIR_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("imaged-test-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    async fn database(&self) -> SqlitePool {
        setup_database(&self.0.join("test.db")).await.unwrap()
    }
}

impl Default for TestDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Repos {
    pub pool: SqlitePool,
    pub host_repo: Arc<dyn HostRepository>,
    pub image_repo: Arc<dyn ImageRepository>,
    pub task_repo: Arc<dyn TaskRepository>,
    pub group_repo: Arc<dyn GroupRepository>,
    pub script_repo: Arc<dyn ScriptRepository>,
}

pub async fn repos() -> (Repos, TestDir) {
    let dir = TestDir::new();
    let pool = dir.database().await;
    let repos = Repos {
        host_repo: repository::host_repo(pool.clone()),
        image_repo: repository::image_repo(pool.clone()),
        task_repo: repository::task_repo(pool.clone()),
        group_repo: repository::group_repo(pool.clone()),
        script_repo: repository::script_repo(pool.clone()),
        pool,
    };
    (repos, dir)
}

pub async fn container() -> (DIContainer, TestDir) {
    let dir = TestDir::new();
    let pool = dir.database().await;
    let mut container = build_di_container(
        pool,
        dir.0.join("images").to_string_lossy().to_string(),
        "lo".to_string(),
        None,
        BTreeMap::new(),
    )
    .await
    .unwrap();
    container.agent_liveness = AgentLiveness {
        ping_interval: Duration::from_millis(200),
        timeout: Duration::from_millis(600),
    };
    (container, dir)
}
