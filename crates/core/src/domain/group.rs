use imaged_model::error::Result;
use imaged_model::group::Group;

#[async_trait::async_trait]
pub trait GroupRepository: Send + Sync {
    async fn create_group(&self, name: &str, host_ids: &[i64]) -> Result<Group>;

    async fn update_name(&self, id: i64, name: &str) -> Result<Group>;

    async fn get_all(&self) -> Result<Vec<Group>>;

    async fn delete(&self, id: i64) -> Result;

    async fn update_group_members(&self, id: i64, host_ids: &[i64]) -> Result<Group>;
}
