use derive_more::Constructor;

use crate::error::Result;

#[derive(Debug, Constructor)]
pub struct Host {
    pub id: i64,
    pub name: String,
    pub mac_address: String,
    pub disk_size: u64,
}

#[async_trait::async_trait]
pub trait HostRepository: Send + Sync {
    async fn upsert_host(&self, mac: String, disk_size_bytes: u64) -> Result<Host>;

    async fn update_name(&self, id: i64, name: String) -> Result<Host>;

    async fn get_all(&self) -> Result<Vec<Host>>;
}
