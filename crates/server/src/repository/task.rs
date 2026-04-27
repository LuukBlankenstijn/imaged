use crate::domain::task::{Task, TaskRepository, TaskState, TaskType};
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
    host_id: Option<i64>,
    image_id: Option<i64>,
    state: String,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    error: Option<String>,
}

impl TryFrom<TaskRow> for Task {
    type Error = crate::error::AppError;

    fn try_from(row: TaskRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            task_type: TaskType::from_string(row.task_type)?,
            host_id: row.host_id,
            image_id: row.image_id,
            state: TaskState::from_string(row.state)?,
            created_at: row.created_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
            error: row.error,
        })
    }
}

#[async_trait::async_trait]
impl TaskRepository for SqliteTaskRepository {
    async fn create(&self, task_type: TaskType, host_id: i64, image_id: i64) -> Result<Task> {
        let type_str = task_type.to_string();
        let state_str = TaskState::Pending.to_string();
        let now = Utc::now();

        let row = sqlx::query_as!(
            TaskRow,
            r#"
            INSERT INTO tasks (type, host_id, image_id, state, created_at)
            VALUES (?, ?, ?, ?, ?)
            RETURNING 
                id as "id!", 
                type as task_type, 
                host_id, 
                image_id, 
                state, 
                created_at as "created_at: DateTime<Utc>", 
                started_at as "started_at: DateTime<Utc>", 
                finished_at as "finished_at: DateTime<Utc>", 
                error
            "#,
            type_str,
            host_id,
            image_id,
            state_str,
            now
        )
        .fetch_one(&self.pool)
        .await?;

        row.try_into()
    }

    async fn get_next(&self, host_id: i64) -> Result<Option<Task>> {
        let running_str = TaskState::Running.to_string();
        let pending_str = TaskState::Pending.to_string();

        let row = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT 
                id as "id!", type as task_type, host_id, image_id, state, 
                created_at as "created_at: DateTime<Utc>", 
                started_at as "started_at: DateTime<Utc>", 
                finished_at as "finished_at: DateTime<Utc>", 
                error
            FROM tasks
            WHERE host_id = ? AND (state = ? OR state = ?)
            ORDER BY 
                CASE state 
                    WHEN ? THEN 1 
                    WHEN ? THEN 2 
                END ASC,
                created_at ASC
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

    async fn reset(&self, id: i64) -> Result {
        let pending_state = TaskState::Pending.to_string();

        sqlx::query!(
            "UPDATE tasks SET state = ?, started_at = NULL, error = NULL WHERE id = ?",
            pending_state,
            id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn start(&self, id: i64) -> Result {
        let running = TaskState::Running.to_string();
        let pending = TaskState::Pending.to_string();
        let now = Utc::now();

        sqlx::query!(
            "UPDATE tasks SET state = ?, started_at = ? WHERE id = ? AND (state = ? OR state = ?)",
            running,
            now,
            id,
            pending,
            running,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn cancel(&self, id: i64) -> Result {
        let cancelled_state = TaskState::Cancelled.to_string();
        let pending_state = TaskState::Pending.to_string();
        let running_state = TaskState::Running.to_string();
        let now = Utc::now();

        sqlx::query!(
            "UPDATE tasks SET state = ?, finished_at = ? WHERE id = ? AND (state = ? OR state = ?)",
            cancelled_state,
            now,
            id,
            pending_state,
            running_state
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
                id as "id!", type as task_type, host_id, image_id, state, 
                created_at as "created_at: DateTime<Utc>", 
                started_at as "started_at: DateTime<Utc>", 
                finished_at as "finished_at: DateTime<Utc>", 
                error
            FROM tasks
            WHERE image_id = ? AND (state = ? OR state = ?)
            ORDER BY created_at DESC
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
                id, type as task_type, host_id, image_id, state, 
                created_at as "created_at: DateTime<Utc>", 
                started_at as "started_at: DateTime<Utc>", 
                finished_at as "finished_at: DateTime<Utc>", 
                error
            FROM tasks
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
                id, type as task_type, host_id, image_id, state, 
                created_at as "created_at: DateTime<Utc>", 
                started_at as "started_at: DateTime<Utc>", 
                finished_at as "finished_at: DateTime<Utc>", 
                error
            FROM tasks
            WHERE id = ?
            "#,
            id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_into()?)
    }

    async fn finish(&self, id: i64) -> Result {
        let state = TaskState::Done.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE tasks SET state = ?, finished_at = ? WHERE id = ?",
            state,
            now,
            id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_failed(&self, id: i64, error: &str) -> Result {
        let state = TaskState::Failed.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE tasks SET state = ?, finished_at = ?, error = ? WHERE id = ?",
            state,
            now,
            error,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
