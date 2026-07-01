use derive_more::Constructor;

use crate::error::Result;

#[derive(Debug, Constructor)]
pub struct Group {
    pub id: i64,
    pub name: String,
}

#[async_trait::async_trait]
pub trait GroupRepository: Send + Sync {
    async fn create_group(&self, name: &str, host_ids: &[i64]) -> Result<Group>;

    async fn update_name(&self, id: i64, name: &str) -> Result<Group>;

    async fn get_all(&self) -> Result<Vec<Group>>;

    async fn delete(&self, id: i64) -> Result;

    async fn update_group_members(&self, id: i64, host_ids: &[i64]) -> Result<Group>;
}
