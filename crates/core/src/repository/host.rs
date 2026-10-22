use derive_more::Constructor;
use sqlx::SqlitePool;

use crate::domain::{self, host::Host};
use imaged_model::{agent::MacAddress, error::Result};

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
    disk_error: Option<String>,
    ip: Option<String>,
}

impl From<HostRow> for Host {
    fn from(value: HostRow) -> Self {
        let disk = match value.disk_error {
            Some(reason) => Err(reason),
            None => Ok(value.disk_size_bytes as u64),
        };
        Self::new(value.id, value.name, value.mac, disk, value.ip)
    }
}

fn canonical_mac(mac: &str) -> Result<String> {
    Ok(mac.parse::<MacAddress>()?.to_string())
}

#[async_trait::async_trait]
impl domain::host::HostRepository for SqliteHostRepository {
    async fn upsert_host(
        &self,
        mac: String,
        disk: std::result::Result<u64, String>,
        ip: Option<String>,
    ) -> Result<Host> {
        let mac = canonical_mac(&mac)?;
        let name = mac.replace(':', "-");
        let (size, disk_error) = match disk {
            Ok(size) => (size as i64, None),
            Err(reason) => (0, Some(reason)),
        };
        let host = sqlx::query_as!(
            HostRow,
            r#"
                INSERT INTO hosts (mac, name, disk_size_bytes, disk_error, ip)
                VALUES (?, ?, ?, ?, ?)
                ON CONFLICT(mac) DO UPDATE SET
                    disk_size_bytes = excluded.disk_size_bytes,
                    disk_error = excluded.disk_error,
                    ip = excluded.ip
                RETURNING
                    id AS "id!: i64",
                    mac AS "mac!: String",
                    name AS "name!: String",
                    disk_size_bytes AS "disk_size_bytes!: i64",
                    disk_error AS "disk_error?: String",
                    ip AS "ip!: String"
            "#,
            mac,
            name,
            size,
            disk_error,
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
                r#"SELECT h.id as "id!", h.name, h.mac, h.disk_size_bytes, h.disk_error, h.ip FROM hosts AS h INNER JOIN group_hosts gh on gh.host_id = id AND  gh.group_id = ? ORDER BY h.name"#,
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
        let mac = canonical_mac(mac)?;
        Ok(sqlx::query_as!(
            HostRow,
            r#"
            SELECT 
                id AS "id!",
                mac,
                name,
                disk_size_bytes,
                disk_error,
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
    use crate::domain::task::TaskType;
    use crate::test_support::repos;
    use imaged_model::error::AppError;
    use imaged_model::task::TaskState;

    #[tokio::test]
    async fn upsert_host_inserts_then_updates_size_and_ip_preserving_id_and_user_assigned_name() {
        let (c, _guard) = repos().await;

        let inserted = c
            .host_repo
            .upsert_host(
                "aa:bb:cc:dd:ee:01".into(),
                Ok(1_000),
                Some("10.0.0.1".into()),
            )
            .await
            .unwrap();
        assert_eq!(inserted.name, "aa-bb-cc-dd-ee-01");
        assert_eq!(inserted.disk, Ok(1_000));
        assert_eq!(inserted.ip.as_deref(), Some("10.0.0.1"));

        let renamed = c
            .host_repo
            .update_name(inserted.id, "workstation-7".into())
            .await
            .unwrap();
        assert_eq!(renamed.name, "workstation-7");

        let reconnected = c
            .host_repo
            .upsert_host(
                "aa:bb:cc:dd:ee:01".into(),
                Ok(2_000),
                Some("10.0.0.2".into()),
            )
            .await
            .unwrap();

        assert_eq!(reconnected.id, inserted.id);
        assert_eq!(reconnected.name, "workstation-7");
        assert_eq!(reconnected.disk, Ok(2_000));
        assert_eq!(reconnected.ip.as_deref(), Some("10.0.0.2"));

        let all = c.host_repo.get_all(None).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn disk_size_round_trips_for_four_tebibytes() {
        let (c, _guard) = repos().await;
        let four_tib = 4u64 * 1024 * 1024 * 1024 * 1024;

        let host = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:02".into(), Ok(four_tib), None)
            .await
            .unwrap();
        assert_eq!(host.disk, Ok(four_tib));

        let fetched = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:02").await.unwrap();
        assert_eq!(fetched.disk, Ok(four_tib));
    }

    #[tokio::test]
    async fn disk_size_u64_max_round_trips() {
        let (c, _guard) = repos().await;

        let host = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:ff".into(), Ok(u64::MAX), None)
            .await
            .unwrap();
        assert_eq!(host.disk, Ok(u64::MAX));

        let fetched = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:ff").await.unwrap();
        assert_eq!(fetched.disk, Ok(u64::MAX));
    }

    #[tokio::test]
    async fn a_host_without_a_usable_disk_keeps_the_reason_until_it_reports_a_disk() {
        let (c, _guard) = repos().await;

        let host = c
            .host_repo
            .upsert_host(
                "aa:bb:cc:dd:ee:10".into(),
                Err("multiple disks found: [\"sda\", \"sdb\"]".into()),
                None,
            )
            .await
            .unwrap();
        let fetched = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:10").await.unwrap();
        assert_eq!(
            fetched.disk,
            Err("multiple disks found: [\"sda\", \"sdb\"]".to_string())
        );
        let err = fetched.usable_disk().unwrap_err();
        assert!(
            matches!(&err, AppError::FailedPrecondition(message)
                if message == "host aa-bb-cc-dd-ee-10 has no usable disk: multiple disks found: [\"sda\", \"sdb\"]"),
            "got {err:?}"
        );

        let fixed = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:10".into(), Ok(500), None)
            .await
            .unwrap();
        assert_eq!(fixed.id, host.id);
        assert_eq!(fixed.usable_disk().unwrap(), 500);
        assert_eq!(c.host_repo.get_all(None).await.unwrap()[0].disk, Ok(500));
    }

    #[tokio::test]
    async fn get_by_mac_returns_the_host() {
        let (c, _guard) = repos().await;
        let host = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:03".into(), Ok(1_000), None)
            .await
            .unwrap();

        let fetched = c.host_repo.get_by_mac("aa:bb:cc:dd:ee:03").await.unwrap();
        assert_eq!(fetched.id, host.id);
        assert_eq!(fetched.mac_address, "aa:bb:cc:dd:ee:03");
    }

    #[tokio::test]
    async fn get_by_mac_unknown_mac_is_not_found() {
        let (c, _guard) = repos().await;
        let err = c
            .host_repo
            .get_by_mac("ff:ff:ff:ff:ff:ff")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn macs_are_stored_and_looked_up_in_lowercase_colon_form_whatever_form_they_arrive_in() {
        let (c, _guard) = repos().await;
        let host = c
            .host_repo
            .upsert_host("AA-BB-CC-DD-EE-04".into(), Ok(1_000), None)
            .await
            .unwrap();
        assert_eq!(host.mac_address, "aa:bb:cc:dd:ee:04");
        assert_eq!(host.name, "aa-bb-cc-dd-ee-04");

        let again = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:04".into(), Ok(2_000), None)
            .await
            .unwrap();
        assert_eq!(again.id, host.id);

        for query in [
            "aa:bb:cc:dd:ee:04",
            "AA:BB:CC:DD:EE:04",
            "aa-bb-cc-dd-ee-04",
        ] {
            assert_eq!(c.host_repo.get_by_mac(query).await.unwrap().id, host.id);
        }
        assert_eq!(c.host_repo.get_all(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn malformed_macs_are_rejected_without_creating_a_host() {
        let (c, _guard) = repos().await;

        let upsert = c.host_repo.upsert_host("foo".into(), Ok(1_000), None).await;
        assert!(
            matches!(upsert, Err(AppError::InvalidArgument(_))),
            "got {upsert:?}"
        );
        let lookup = c.host_repo.get_by_mac("aa:bb:cc:dd:ee").await;
        assert!(
            matches!(lookup, Err(AppError::InvalidArgument(_))),
            "got {lookup:?}"
        );
        assert!(c.host_repo.get_all(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_all_returns_hosts_ordered_by_name() {
        let (c, _guard) = repos().await;
        let charlie = c
            .host_repo
            .upsert_host("cc:bb:cc:dd:ee:05".into(), Ok(1), None)
            .await
            .unwrap();
        let alpha = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:06".into(), Ok(2), None)
            .await
            .unwrap();
        let bravo = c
            .host_repo
            .upsert_host("bb:bb:cc:dd:ee:07".into(), Ok(3), None)
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
        let (c, _guard) = repos().await;
        let a = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:08".into(), Ok(1), None)
            .await
            .unwrap();
        let b = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:09".into(), Ok(2), None)
            .await
            .unwrap();
        let d = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:0a".into(), Ok(3), None)
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
        let (c, _guard) = repos().await;
        let host = c
            .host_repo
            .upsert_host(
                "aa:bb:cc:dd:ee:0b".into(),
                Ok(4_096),
                Some("10.0.0.9".into()),
            )
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
        assert_eq!(renamed.disk, Ok(4_096));
        assert_eq!(renamed.ip.as_deref(), Some("10.0.0.9"));
    }

    #[tokio::test]
    async fn update_name_unknown_id_is_not_found() {
        let (c, _guard) = repos().await;
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
        let (c, _guard) = repos().await;
        let a = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:0c".into(), Ok(1_000), None)
            .await
            .unwrap();
        let b = c
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:0d".into(), Ok(1_000), None)
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
