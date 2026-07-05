use crate::domain::task::{Task, TaskHost, TaskRepository, TaskState, TaskType};
use crate::error::Result;
use chrono::{DateTime, Utc};
use derive_more::Constructor;
use sqlx::SqlitePool;

#[derive(Debug, Constructor)]
pub struct SqliteTaskRepository {
    pool: SqlitePool,
}

#[derive(sqlx::FromRow)]
struct TaskRow {
    id: i64,
    task_type: String,
    image_id: Option<i64>,
    image_name: Option<String>,
    image_deleted: i64,
    created_at: DateTime<Utc>,
    hosts: sqlx::types::Json<Vec<TaskHost>>,
}

impl TryFrom<TaskRow> for Task {
    type Error = crate::error::AppError;

    fn try_from(row: TaskRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            task_type: TaskType::from_string(row.task_type)?,
            hosts: row.hosts.0,
            image_id: row.image_id,
            image_name: row.image_name,
            image_deleted: row.image_deleted != 0,
            created_at: row.created_at,
        })
    }
}

#[async_trait::async_trait]
impl TaskRepository for SqliteTaskRepository {
    async fn create(
        &self,
        task_type: TaskType,
        host_ids: Vec<i64>,
        image_id: Option<i64>,
    ) -> Result<Task> {
        let type_str = task_type.to_string();
        let pending_str = TaskState::Pending.to_string();
        let now = Utc::now();

        let task = sqlx::query!(
            r#"
            INSERT INTO tasks (type, image_id, created_at)
            VALUES (?, ?, ?)
            RETURNING id as "id!"
            "#,
            type_str,
            image_id,
            now
        )
        .fetch_one(&self.pool)
        .await?;

        let mut tx = self.pool.begin().await?;
        for host_id in host_ids.iter() {
            sqlx::query!(
                "INSERT INTO task_hosts (task_id, host_id, state) VALUES (?, ?, ?)",
                task.id,
                host_id,
                pending_str
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        self.get(task.id).await
    }

    async fn get_next(&self, host_id: i64) -> Result<Option<Task>> {
        let running_str = TaskState::Running.to_string();
        let pending_str = TaskState::Pending.to_string();

        let row = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT
                twh.id as "id!", twh.type as task_type, twh.image_id, twh.image_name,
                twh.image_deleted as "image_deleted!: i64",
                twh.created_at as "created_at: DateTime<Utc>",
                twh.hosts as "hosts!: sqlx::types::Json<Vec<TaskHost>>"
            FROM tasks_with_hosts twh
            JOIN task_hosts th ON th.task_id = twh.id
            WHERE th.host_id = ?
            AND (th.state = ? OR th.state = ?)
            ORDER BY
                CASE th.state
                    WHEN ? THEN 1
                    WHEN ? THEN 2
                END ASC,
                twh.created_at ASC
            LIMIT 1
            "#,
            host_id,
            running_str,
            pending_str,
            running_str,
            pending_str
        )
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(r.try_into()?)),
            None => Ok(None),
        }
    }

    async fn start(&self, task_id: i64, host_id: i64) -> Result {
        let running = TaskState::Running.to_string();
        let pending = TaskState::Pending.to_string();
        let now = Utc::now();

        sqlx::query!(
            "UPDATE task_hosts SET state = ?, started_at = ? \
             WHERE task_id = ? AND host_id = ? AND (state = ? OR state = ?)",
            running,
            now,
            task_id,
            host_id,
            pending,
            running,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_finished(&self, task_id: i64, host_id: i64) -> Result {
        let state = TaskState::Done.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE task_hosts SET state = ?, finished_at = ? WHERE task_id = ? AND host_id = ?",
            state,
            now,
            task_id,
            host_id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_failed(&self, task_id: i64, host_id: i64, error: &str) -> Result {
        let state = TaskState::Failed.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE task_hosts SET state = ?, finished_at = ?, error = ? \
             WHERE task_id = ? AND host_id = ?",
            state,
            now,
            error,
            task_id,
            host_id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_all_finished(&self, task_id: i64) -> Result {
        let done = TaskState::Done.to_string();
        let pending = TaskState::Pending.to_string();
        let running = TaskState::Running.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE task_hosts SET state = ?, finished_at = ? \
             WHERE task_id = ? AND (state = ? OR state = ?)",
            done,
            now,
            task_id,
            pending,
            running
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_all_failed(&self, task_id: i64, error: &str) -> Result {
        let failed = TaskState::Failed.to_string();
        let pending = TaskState::Pending.to_string();
        let running = TaskState::Running.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE task_hosts SET state = ?, finished_at = ?, error = ? \
             WHERE task_id = ? AND (state = ? OR state = ?)",
            failed,
            now,
            error,
            task_id,
            pending,
            running
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn retry(&self, id: i64) -> Result {
        let pending = TaskState::Pending.to_string();
        let failed = TaskState::Failed.to_string();
        let cancelled = TaskState::Cancelled.to_string();

        sqlx::query!(
            "UPDATE task_hosts SET state = ?, error = NULL, started_at = NULL, finished_at = NULL \
             WHERE task_id = ? AND (state = ? OR state = ?)",
            pending,
            id,
            failed,
            cancelled
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn cancel(&self, id: i64) -> Result {
        let cancelled = TaskState::Cancelled.to_string();
        let pending = TaskState::Pending.to_string();
        let running = TaskState::Running.to_string();
        let now = Utc::now();

        sqlx::query!(
            "UPDATE task_hosts SET state = ?, finished_at = ? \
             WHERE task_id = ? AND (state = ? OR state = ?)",
            cancelled,
            now,
            id,
            pending,
            running
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_active_by_image(&self, image_id: i64) -> Result<Vec<Task>> {
        let pending = TaskState::Pending.to_string();
        let running = TaskState::Running.to_string();

        let rows = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT
                twh.id as "id!", twh.type as task_type, twh.image_id, twh.image_name,
                twh.image_deleted as "image_deleted!: i64",
                twh.created_at as "created_at: DateTime<Utc>",
                twh.hosts as "hosts!: sqlx::types::Json<Vec<TaskHost>>"
            FROM tasks_with_hosts twh
            WHERE twh.image_id = ?
            AND EXISTS (
                SELECT 1 FROM task_hosts th
                WHERE th.task_id = twh.id AND (th.state = ? OR th.state = ?)
            )
            ORDER BY twh.created_at DESC
            "#,
            image_id,
            pending,
            running
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.try_into()).collect()
    }

    /// Returns every task in the database, no filters.
    async fn get_all(&self) -> Result<Vec<Task>> {
        let rows = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT
                id as "id!", type as task_type, image_id, image_name,
                image_deleted as "image_deleted!: i64",
                created_at as "created_at: DateTime<Utc>",
                hosts as "hosts!: sqlx::types::Json<Vec<TaskHost>>"
            FROM tasks_with_hosts
            ORDER BY created_at DESC
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.try_into()).collect()
    }

    async fn get(&self, id: i64) -> Result<Task> {
        let row = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT
                id as "id!", type as task_type, image_id, image_name,
                image_deleted as "image_deleted!: i64",
                created_at as "created_at: DateTime<Utc>",
                hosts as "hosts!: sqlx::types::Json<Vec<TaskHost>>"
            FROM tasks_with_hosts
            WHERE id = ?
            "#,
            id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_into()?)
    }

    async fn get_next_multicast(&self) -> Result<Option<Task>> {
        let pending_state = TaskState::Pending.to_string();
        let task_type = TaskType::Multicast.to_string();
        let row = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT
                twh.id as "id!", twh.type as task_type, twh.image_id, twh.image_name,
                twh.image_deleted as "image_deleted!: i64",
                twh.created_at as "created_at: DateTime<Utc>",
                twh.hosts as "hosts!: sqlx::types::Json<Vec<TaskHost>>"
            FROM tasks_with_hosts twh
            WHERE twh.type = ?
            AND EXISTS (
                SELECT 1 FROM task_hosts th
                WHERE th.task_id = twh.id AND th.state = ?
            )
            "#,
            task_type,
            pending_state
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.try_into()).transpose()?)
    }
}
