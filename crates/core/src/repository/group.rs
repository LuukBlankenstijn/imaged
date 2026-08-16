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
