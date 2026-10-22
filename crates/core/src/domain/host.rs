use derive_more::Constructor;

use imaged_model::error::{AppError, Result};

#[derive(Debug, Constructor)]
pub struct Host {
    pub id: i64,
    pub name: String,
    pub mac_address: String,
    pub disk: std::result::Result<u64, String>,
    pub ip: Option<String>,
}

impl Host {
    pub fn usable_disk(&self) -> Result<u64> {
        self.disk.as_ref().copied().map_err(|reason| {
            AppError::FailedPrecondition(format!("host {} has no usable disk: {reason}", self.name))
        })
    }
}

#[async_trait::async_trait]
pub trait HostRepository: Send + Sync {
    async fn upsert_host(
        &self,
        mac: String,
        disk: std::result::Result<u64, String>,
        ip: Option<String>,
    ) -> Result<Host>;

    async fn update_name(&self, id: i64, name: String) -> Result<Host>;

    async fn get_all(&self, group_id: Option<i64>) -> Result<Vec<Host>>;

    async fn delete(&self, id: i64) -> Result;

    async fn get_by_mac(&self, mac: &str) -> Result<Host>;
}
