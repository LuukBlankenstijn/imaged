use std::sync::Arc;

use sqlx::SqlitePool;

use crate::domain::{
    group::GroupRepository, host::HostRepository, image::ImageRepository, task::TaskRepository,
};

mod group;
mod host;
mod image;
mod task;

pub fn host_repo(pool: SqlitePool) -> Arc<dyn HostRepository> {
    Arc::new(host::SqliteHostRepository::new(pool))
}

pub fn image_repo(pool: SqlitePool) -> Arc<dyn ImageRepository> {
    Arc::new(image::SqliteImageRepository::new(pool))
}

pub fn task_repo(pool: SqlitePool) -> Arc<dyn TaskRepository> {
    Arc::new(task::SqliteTaskRepository::new(pool))
}

pub fn group_repo(pool: SqlitePool) -> Arc<dyn GroupRepository> {
    Arc::new(group::SqliteGroupRepository::new(pool))
}
