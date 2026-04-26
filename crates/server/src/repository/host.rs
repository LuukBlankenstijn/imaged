use derive_more::Constructor;
use sqlx::SqlitePool;

use crate::{
    domain::{self, host::Host},
    error::Result,
};

#[derive(Debug, Constructor)]
pub struct SqliteHostRepository {
    pool: SqlitePool,
}

#[derive(sqlx::FromRow)]
struct HostRow {
    id: i64,
    mac: String,
    name: String,
    disk_size_bytes: i64,
}

impl From<HostRow> for Host {
    fn from(value: HostRow) -> Self {
        Self::new(
            value.id,
            value.name,
            value.mac,
            value.disk_size_bytes as u64,
        )
    }
}

#[async_trait::async_trait]
impl domain::host::HostRepository for SqliteHostRepository {
    async fn upsert_host(&self, mac_address: String, disk_size_bytes: u64) -> Result<Host> {
        let name = mac_address.replace(":", "-");
        let size = disk_size_bytes as i64;
        let host = sqlx::query_as!(
            HostRow,
            r#"
                INSERT INTO hosts (mac, name, disk_size_bytes)
                VALUES (?, ?, ?)
                ON CONFLICT(mac) DO UPDATE SET
                    disk_size_bytes = excluded.disk_size_bytes
                RETURNING
                    id AS "id!: i64",
                    mac AS "mac!: String",
                    name AS "name!: String",
                    disk_size_bytes AS "disk_size_bytes!: i64"
            "#,
            mac_address,
            name,
            size
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(host.into())
    }

    async fn update_name(&self, id: i64, name: String) -> Result<Host> {
        let host = sqlx::query_as!(
            HostRow,
            "UPDATE hosts set name = ? WHERE id = ? RETURNING *",
            name,
            id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(host.into())
    }

    async fn get_all(&self) -> Result<Vec<Host>> {
        Ok(sqlx::query_as!(HostRow, "SELECT * FROM hosts")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| row.into())
            .collect())
    }
}
