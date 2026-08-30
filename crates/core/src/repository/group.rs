use crate::error::Result;
use derive_more::Constructor;
use sqlx::SqlitePool;

use crate::domain::group::{Group, GroupRepository};

#[derive(Debug, Constructor)]
pub struct SqliteGroupRepository {
    pool: SqlitePool,
}

#[async_trait::async_trait]
impl GroupRepository for SqliteGroupRepository {
    async fn create_group(&self, name: &str, host_ids: &[i64]) -> Result<Group> {
        let mut tx = self.pool.begin().await?;
        let group = sqlx::query!("INSERT INTO groups (name) VALUES (?) RETURNING *", name)
            .fetch_one(&mut *tx)
            .await?;

        for host_id in host_ids {
            sqlx::query!(
                "INSERT INTO group_hosts (host_id, group_id) VALUES (?, ?)",
                host_id,
                group.id,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        Ok(Group {
            id: group.id,
            name: group.name,
        })
    }

    async fn update_name(&self, id: i64, name: &str) -> Result<Group> {
        let group = sqlx::query!(
            "UPDATE groups SET name = ? WHERE id = ? RETURNING *",
            name,
            id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(Group {
            id,
            name: group.name,
        })
    }

    async fn get_all(&self) -> Result<Vec<Group>> {
        Ok(sqlx::query!("SELECT * FROM groups")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| Group {
                id: r.id,
                name: r.name,
            })
            .collect())
    }

    async fn delete(&self, id: i64) -> Result {
        sqlx::query!("DELETE FROM groups WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_group_members(&self, id: i64, host_ids: &[i64]) -> Result<Group> {
        let mut tx = self.pool.begin().await?;
        // first get the group to check if it exists
        let group = sqlx::query!("SELECT * FROM groups WHERE id = ?", id)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query!("DELETE from group_hosts WHERE group_id = ?", id)
            .execute(&mut *tx)
            .await?;
        for host_id in host_ids {
            sqlx::query!(
                "INSERT INTO group_hosts (host_id, group_id) VALUES (?, ?)",
                host_id,
                id
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        Ok(Group {
            id,
            name: group.name,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::build_test_container;
    use crate::di::DIContainer;
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
            std::env::temp_dir().join(format!("imaged-repogroup-{}-{}", std::process::id(), id));
        let c = build_test_container(&dir).await;
        (c, TestDir(dir))
    }

    async fn host(c: &DIContainer, mac: &str) -> i64 {
        c.host_repo
            .upsert_host(mac.into(), 1_000, None)
            .await
            .unwrap()
            .id
    }

    async fn member_ids(c: &DIContainer, group_id: i64) -> Vec<i64> {
        let mut ids: Vec<i64> = c
            .host_repo
            .get_all(Some(group_id))
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn create_group_round_trips_through_get_all() {
        let (c, _guard) = container().await;
        let created = c.group_repo.create_group("alpha", &[]).await.unwrap();
        assert_eq!(created.name, "alpha");

        let all = c.group_repo.get_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, created.id);
        assert_eq!(all[0].name, "alpha");
    }

    #[tokio::test]
    async fn create_group_persists_its_initial_membership() {
        let (c, _guard) = container().await;
        let a = host(&c, "aa:bb:cc:00:00:01").await;
        let b = host(&c, "aa:bb:cc:00:00:02").await;

        let group = c.group_repo.create_group("g", &[a, b]).await.unwrap();

        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(member_ids(&c, group.id).await, expected);
    }

    #[tokio::test]
    async fn get_all_returns_every_group_but_the_sql_has_no_order_by() {
        let (c, _guard) = container().await;
        let x = c.group_repo.create_group("x", &[]).await.unwrap();
        let y = c.group_repo.create_group("y", &[]).await.unwrap();
        let z = c.group_repo.create_group("z", &[]).await.unwrap();

        let mut got: Vec<i64> = c
            .group_repo
            .get_all()
            .await
            .unwrap()
            .into_iter()
            .map(|g| g.id)
            .collect();
        got.sort();
        let mut expected = vec![x.id, y.id, z.id];
        expected.sort();
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn update_name_unknown_id_is_not_found() {
        let (c, _guard) = container().await;
        let err = c.group_repo.update_name(999_999, "nope").await.unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn update_group_members_replaces_the_membership_set() {
        let (c, _guard) = container().await;
        let a = host(&c, "aa:bb:cc:00:01:01").await;
        let b = host(&c, "aa:bb:cc:00:01:02").await;
        let d = host(&c, "aa:bb:cc:00:01:03").await;

        let group = c.group_repo.create_group("g", &[a, b]).await.unwrap();

        c.group_repo
            .update_group_members(group.id, &[b, d])
            .await
            .unwrap();

        let mut expected = vec![b, d];
        expected.sort();
        assert_eq!(member_ids(&c, group.id).await, expected);
    }

    #[tokio::test]
    async fn update_group_members_with_an_empty_list_clears_membership() {
        let (c, _guard) = container().await;
        let a = host(&c, "aa:bb:cc:00:02:01").await;
        let group = c.group_repo.create_group("g", &[a]).await.unwrap();
        assert_eq!(member_ids(&c, group.id).await, vec![a]);

        c.group_repo
            .update_group_members(group.id, &[])
            .await
            .unwrap();
        assert!(member_ids(&c, group.id).await.is_empty());
    }

    #[tokio::test]
    async fn update_group_members_with_a_duplicate_host_id_fails_on_the_unique_constraint_and_rolls_back()
     {
        let (c, _guard) = container().await;
        let a = host(&c, "aa:bb:cc:00:03:01").await;
        let b = host(&c, "aa:bb:cc:00:03:02").await;
        let group = c.group_repo.create_group("g", &[a]).await.unwrap();

        let err = c
            .group_repo
            .update_group_members(group.id, &[b, b])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::AlreadyExists(_)), "got {err:?}");

        assert_eq!(member_ids(&c, group.id).await, vec![a]);
    }

    #[tokio::test]
    async fn update_group_members_with_a_nonexistent_host_id_fails_the_foreign_key_and_rolls_back_leaving_prior_membership_intact()
     {
        let (c, _guard) = container().await;
        let a = host(&c, "aa:bb:cc:00:04:01").await;
        let b = host(&c, "aa:bb:cc:00:04:02").await;
        let group = c.group_repo.create_group("g", &[a]).await.unwrap();

        let err = c
            .group_repo
            .update_group_members(group.id, &[b, 999_999])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::FailedPrecondition(_)), "got {err:?}");

        assert_eq!(member_ids(&c, group.id).await, vec![a]);
    }

    #[tokio::test]
    async fn update_group_members_unknown_group_id_is_not_found() {
        let (c, _guard) = container().await;
        let err = c
            .group_repo
            .update_group_members(999_999, &[])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn delete_group_removes_the_group_and_its_membership_without_deleting_the_hosts() {
        let (c, _guard) = container().await;
        let a = host(&c, "aa:bb:cc:00:05:01").await;
        let b = host(&c, "aa:bb:cc:00:05:02").await;
        let group = c.group_repo.create_group("g", &[a, b]).await.unwrap();

        c.group_repo.delete(group.id).await.unwrap();

        assert!(c.group_repo.get_all().await.unwrap().is_empty());

        let mut host_ids: Vec<i64> = c
            .host_repo
            .get_all(None)
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        host_ids.sort();
        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(host_ids, expected);
    }
}
