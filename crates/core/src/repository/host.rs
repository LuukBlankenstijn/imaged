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
                r#"SELECT h.id as "id!", h.name, h.mac, h.disk_size_bytes, h.ip FROM hosts AS h INNER JOIN group_hosts gh on gh.host_id = id AND  gh.group_id = ? ORDER BY h.name"#,
                group_id
            ).fetch_all(&self.pool)
            .await?,
            None => sqlx::query_as!(HostRow, "SELECT * FROM hosts ORDER BY name").fetch_all(&self.pool)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::build_test_container;
    use crate::di::DIContainer;
    use crate::domain::task::{TaskState, TaskType};
    use crate::error::AppError;

    static DB_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn container() -> (DIContainer, TestDir) {
        let id = DB_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("imaged-repohost-{}-{}", std::process::id(), id));
        let c = build_test_container(&dir).await;
        (c, TestDir(dir))
    }

    #[tokio::test]
    async fn upsert_host_inserts_then_updates_size_and_ip_preserving_id_and_user_assigned_name() {
        let (c, _guard) = container().await;

        let inserted = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:01".into(), 1_000, Some("10.0.0.1".into()))
            .await
            .unwrap();
        assert_eq!(inserted.name, "aa-bb-cc-dd-ee-01");
        assert_eq!(inserted.disk_size, 1_000);
        assert_eq!(inserted.ip.as_deref(), Some("10.0.0.1"));

        let renamed = c
            .host_repo
            .update_name(inserted.id, "workstation-7".into())
            .await
            .unwrap();
        assert_eq!(renamed.name, "workstation-7");

        let reconnected = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:01".into(), 2_000, Some("10.0.0.2".into()))
            .await
            .unwrap();

        assert_eq!(reconnected.id, inserted.id);
        assert_eq!(reconnected.name, "workstation-7");
        assert_eq!(reconnected.disk_size, 2_000);
        assert_eq!(reconnected.ip.as_deref(), Some("10.0.0.2"));

        let all = c.host_repo.get_all(None).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn disk_size_round_trips_for_four_tebibytes() {
        let (c, _guard) = container().await;
        let four_tib = 4u64 * 1024 * 1024 * 1024 * 1024;

        let host = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:02".into(), four_tib, None)
            .await
            .unwrap();
        assert_eq!(host.disk_size, four_tib);

        let fetched = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:02").await.unwrap();
        assert_eq!(fetched.disk_size, four_tib);
    }

    #[tokio::test]
    async fn disk_size_u64_max_round_trips_but_is_stored_as_a_negative_i64() {
        let (c, guard) = container().await;

        let host = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:ff".into(), u64::MAX, None)
            .await
            .unwrap();
        assert_eq!(host.disk_size, u64::MAX);

        let fetched = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:ff").await.unwrap();
        assert_eq!(fetched.disk_size, u64::MAX);

        let db_path = guard.0.join("test.db");
        let ro = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=ro", db_path.display()))
            .await
            .unwrap();
        let raw: i64 = sqlx::query_scalar("SELECT disk_size_bytes FROM hosts WHERE mac = ?")
            .bind("aa:bb:cc:dd:ee:ff")
            .fetch_one(&ro)
            .await
            .unwrap();
        ro.close().await;
        assert_eq!(raw, -1);
    }

    #[tokio::test]
    async fn get_by_mac_returns_the_host() {
        let (c, _guard) = container().await;
        let host = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:03".into(), 1_000, None)
            .await
            .unwrap();

        let fetched = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:03").await.unwrap();
        assert_eq!(fetched.id, host.id);
        assert_eq!(fetched.mac_address, "aa:bb:cc:dd:ee:03");
    }

    #[tokio::test]
    async fn get_by_mac_unknown_mac_is_not_found() {
        let (c, _guard) = container().await;
        let err = c
            .host_repo
            .get_by_mac("ff:ff:ff:ff:ff:ff")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn get_by_mac_is_case_sensitive_so_a_lowercase_query_misses_an_uppercase_stored_mac() {
        let (c, _guard) = container().await;
        c.host_repo
            .upsert_host("AA:BB:CC:DD:EE:04".into(), 1_000, None)
            .await
            .unwrap();

        let miss = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:04").await;
        assert!(matches!(miss, Err(AppError::NotFound(_))), "got {miss:?}");

        let hit = c.host_repo.get_by_mac("AA:BB:CC:DD:EE:04").await.unwrap();
        assert_eq!(hit.mac_address, "AA:BB:CC:DD:EE:04");
    }

    #[tokio::test]
    async fn get_all_returns_hosts_ordered_by_name() {
        let (c, _guard) = container().await;
        let charlie = c
            .host_repo
            .upsert_host("cc:bb:cc:dd:ee:05".into(), 1, None)
            .await
            .unwrap();
        let alpha = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:06".into(), 2, None)
            .await
            .unwrap();
        let bravo = c
            .host_repo
            .upsert_host("bb:bb:cc:dd:ee:07".into(), 3, None)
            .await
            .unwrap();

        let got: Vec<i64> = c
            .host_repo
            .get_all(None)
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(got, vec![alpha.id, bravo.id, charlie.id]);
    }

    #[tokio::test]
    async fn get_all_with_group_id_returns_only_that_groups_members() {
        let (c, _guard) = container().await;
        let a = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:08".into(), 1, None)
            .await
            .unwrap();
        let b = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:09".into(), 2, None)
            .await
            .unwrap();
        let d = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:0a".into(), 3, None)
            .await
            .unwrap();

        let group = c.group_repo.create_group("g", &[a.id, d.id]).await.unwrap();

        let mut got: Vec<i64> = c
            .host_repo
            .get_all(Some(group.id))
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        got.sort();
        let mut expected = vec![a.id, d.id];
        expected.sort();
        assert_eq!(got, expected);
        assert!(!got.contains(&b.id));
    }

    #[tokio::test]
    async fn update_name_renames_without_touching_mac_disk_or_ip() {
        let (c, _guard) = container().await;
        let host = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:0b".into(), 4_096, Some("10.0.0.9".into()))
            .await
            .unwrap();

        let renamed = c
            .host_repo
            .update_name(host.id, "lab-01".into())
            .await
            .unwrap();
        assert_eq!(renamed.id, host.id);
        assert_eq!(renamed.name, "lab-01");
        assert_eq!(renamed.mac_address, "aa:bb:cc:dd:ee:0b");
        assert_eq!(renamed.disk_size, 4_096);
        assert_eq!(renamed.ip.as_deref(), Some("10.0.0.9"));
    }

    #[tokio::test]
    async fn update_name_unknown_id_is_not_found() {
        let (c, _guard) = container().await;
        let err = c
            .host_repo
            .update_name(999_999, "nope".into())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn delete_removes_the_host_and_cascades_task_hosts_leaving_a_defunct_task_row_aggregating_to_cancelled()
     {
        let (c, _guard) = container().await;
        let a = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:0c".into(), 1_000, None)
            .await
            .unwrap();
        let b = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:0d".into(), 1_000, None)
            .await
            .unwrap();

        let task = c
            .task_repo
            .create(TaskType::Deploy, vec![a.id], None)
            .await
            .unwrap();
        assert_eq!(task.hosts.len(), 1);

        c.host_repo.delete(a.id).await.unwrap();

        let gone = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:0c").await;
        assert!(matches!(gone, Err(AppError::NotFound(_))), "got {gone:?}");
        let survivor = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:0d").await.unwrap();
        assert_eq!(survivor.id, b.id);

        let defunct = c.task_repo.get(task.id).await.unwrap();
        assert!(defunct.hosts.is_empty());
        assert_eq!(defunct.aggregate_state(), TaskState::Cancelled);
    }
}
