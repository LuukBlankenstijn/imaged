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
    ip: Option<String>,
}

impl From<HostRow> for Host {
    fn from(value: HostRow) -> Self {
        Self::new(
            value.id,
            value.name,
            value.mac,
            value.disk_size_bytes as u64,
            value.ip,
        )
    }
}

#[async_trait::async_trait]
impl domain::host::HostRepository for SqliteHostRepository {
    async fn upsert_host(
        &self,
        mac_address: String,
        disk_size_bytes: u64,
        ip: Option<String>,
    ) -> Result<Host> {
        let name = mac_address.replace(":", "-");
        let size = disk_size_bytes as i64;
        let host = sqlx::query_as!(
            HostRow,
            r#"
                INSERT INTO hosts (mac, name, disk_size_bytes, ip)
                VALUES (?, ?, ?, ?)
                ON CONFLICT(mac) DO UPDATE SET
                    disk_size_bytes = excluded.disk_size_bytes,
                    ip = excluded.ip
                RETURNING
                    id AS "id!: i64",
                    mac AS "mac!: String",
                    name AS "name!: String",
                    disk_size_bytes AS "disk_size_bytes!: i64",
                    ip AS "ip!: String"
            "#,
            mac_address,
            name,
            size,
            ip
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

    async fn get_all(&self, group_id: Option<i64>) -> Result<Vec<Host>> {
        let host_rows = match group_id {
            Some(group_id) => sqlx::query_as!(
                HostRow,
                r#"SELECT h.id as "id!", h.name, h.mac, h.disk_size_bytes, h.ip FROM hosts AS h INNER JOIN group_hosts gh on gh.host_id = id AND  gh.group_id = ?"#,
                group_id
            ).fetch_all(&self.pool)
            .await?,
            None => sqlx::query_as!(HostRow, "SELECT * FROM hosts").fetch_all(&self.pool)
            .await?,
        };
        Ok(host_rows.into_iter().map(|row| row.into()).collect())
    }

    async fn delete(&self, id: i64) -> Result {
        sqlx::query_as!(HostRow, "DELETE FROM hosts WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_by_mac(&self, mac: &str) -> Result<Host> {
        Ok(sqlx::query_as!(
            HostRow,
            r#"
            SELECT 
                id AS "id!",
                mac,
                name,
                disk_size_bytes,
                ip
            FROM hosts 
            WHERE mac = ?
            "#,
            mac
        )
        .fetch_one(&self.pool)
        .await?
        .into())
    }
}
