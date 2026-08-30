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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::di::DIContainer;
    use crate::error::AppError;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

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
            std::env::temp_dir().join(format!("imaged-repotask-{}-{}", std::process::id(), id));
        let c = crate::build_test_container(&dir).await;
        (c, TestDir(dir))
    }

    async fn host(c: &DIContainer, mac: &str) -> i64 {
        c.host_repo
            .upsert_host(mac.into(), 1_000_000, None)
            .await
            .unwrap()
            .id
    }

    async fn image(c: &DIContainer, name: &str) -> i64 {
        c.image_repo.create_image(name.into()).await.unwrap().id
    }

    fn host_row(task: &Task, host_id: i64) -> &TaskHost {
        task.hosts
            .iter()
            .find(|h| h.host_id == host_id)
            .expect("host row present on task")
    }

    #[tokio::test]
    async fn create_inserts_a_pending_row_for_each_host_and_returns_them_attached() {
        let (c, _g) = container().await;
        let img = image(&c, "img").await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;

        let task = c
            .task_repo
            .create(TaskType::Deploy, vec![h1, h2], Some(img))
            .await
            .unwrap();

        assert_eq!(task.task_type, TaskType::Deploy);
        assert_eq!(task.image_id, Some(img));
        assert_eq!(task.hosts.len(), 2);
        for hid in [h1, h2] {
            let r = host_row(&task, hid);
            assert_eq!(r.state, TaskState::Pending);
            assert!(r.error.is_none());
            assert!(r.started_at.is_none());
            assert!(r.finished_at.is_none());
        }
        assert_eq!(task.aggregate_state(), TaskState::Pending);
    }

    #[tokio::test]
    async fn create_with_an_empty_host_list_yields_a_task_that_aggregates_to_cancelled() {
        let (c, _g) = container().await;
        let task = c
            .task_repo
            .create(TaskType::Reboot, vec![], None)
            .await
            .unwrap();
        assert!(task.hosts.is_empty());
        assert_eq!(task.aggregate_state(), TaskState::Cancelled);

        let fetched = c.task_repo.get(task.id).await.unwrap();
        assert!(fetched.hosts.is_empty());
    }

    #[tokio::test]
    async fn create_with_a_nonexistent_host_id_returns_an_error() {
        let (c, _g) = container().await;
        let result = c
            .task_repo
            .create(TaskType::Reboot, vec![999_999], None)
            .await;
        assert!(result.is_err(), "expected a foreign-key violation error");
    }

    #[tokio::test]
    #[ignore = "create inserts the tasks row on the pool before the task_hosts transaction, so a FK failure rolls back only task_hosts and leaves an orphaned task row behind"]
    async fn create_with_a_nonexistent_host_id_leaves_no_orphaned_task_row_behind() {
        let (c, _g) = container().await;
        let _ = c
            .task_repo
            .create(TaskType::Reboot, vec![999_999], None)
            .await;
        let all = c.task_repo.get_all().await.unwrap();
        assert!(all.is_empty(), "a failed create must not leave a task row behind");
    }

    #[tokio::test]
    async fn get_returns_the_task_with_its_host_rows() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let created = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();

        let got = c.task_repo.get(created.id).await.unwrap();
        assert_eq!(got.id, created.id);
        assert_eq!(got.hosts.len(), 1);
        assert_eq!(host_row(&got, h).state, TaskState::Pending);
    }

    #[tokio::test]
    async fn get_maps_a_missing_task_id_to_not_found() {
        let (c, _g) = container().await;
        let err = c.task_repo.get(999_999).await.unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn get_all_returns_tasks_newest_first_with_host_rows_grouped_per_task() {
        let (c, _g) = container().await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;

        let t1 = c
            .task_repo
            .create(TaskType::Reboot, vec![h1], None)
            .await
            .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let t2 = c
            .task_repo
            .create(TaskType::Deploy, vec![h1, h2], None)
            .await
            .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let t3 = c
            .task_repo
            .create(TaskType::Reboot, vec![h2], None)
            .await
            .unwrap();

        let all = c.task_repo.get_all().await.unwrap();
        let ids: Vec<i64> = all.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![t3.id, t2.id, t1.id]);

        let middle = all.iter().find(|t| t.id == t2.id).unwrap();
        let mut middle_hosts: Vec<i64> = middle.hosts.iter().map(|h| h.host_id).collect();
        middle_hosts.sort();
        assert_eq!(middle_hosts, vec![h1, h2]);

        let newest = &all[0];
        assert_eq!(newest.hosts.len(), 1);
        assert_eq!(newest.hosts[0].host_id, h2);
    }

    #[tokio::test]
    async fn get_next_prefers_running_over_pending_then_oldest_created_first() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;

        let t1 = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let t2 = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let t3 = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();

        assert_eq!(c.task_repo.get_next(h).await.unwrap().unwrap().id, t1.id);

        c.task_repo.start(t3.id, h).await.unwrap();
        assert_eq!(c.task_repo.get_next(h).await.unwrap().unwrap().id, t3.id);

        c.task_repo.mark_finished(t3.id, h).await.unwrap();
        assert_eq!(c.task_repo.get_next(h).await.unwrap().unwrap().id, t1.id);

        c.task_repo.mark_finished(t1.id, h).await.unwrap();
        assert_eq!(c.task_repo.get_next(h).await.unwrap().unwrap().id, t2.id);
    }

    #[tokio::test]
    async fn get_next_returns_none_when_the_host_has_only_terminal_tasks() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.mark_finished(t.id, h).await.unwrap();
        assert!(c.task_repo.get_next(h).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_next_returns_none_for_a_host_with_no_tasks() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:09").await;
        assert!(c.task_repo.get_next(h).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn start_transitions_a_pending_host_row_to_running_and_stamps_started_at() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        let r = host_row(&got, h);
        assert_eq!(r.state, TaskState::Running);
        assert!(r.started_at.is_some());
    }

    #[tokio::test]
    async fn start_leaves_an_already_running_host_row_running() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, h).state, TaskState::Running);
    }

    #[tokio::test]
    async fn start_does_not_resurrect_a_done_host_row() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.mark_finished(t.id, h).await.unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, h).state, TaskState::Done);
    }

    #[tokio::test]
    async fn start_does_not_resurrect_a_failed_host_row() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.mark_failed(t.id, h, "boom").await.unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, h).state, TaskState::Failed);
    }

    #[tokio::test]
    async fn start_does_not_resurrect_a_cancelled_host_row() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.cancel(t.id).await.unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, h).state, TaskState::Cancelled);
    }

    #[tokio::test]
    async fn mark_finished_sets_the_host_row_done_and_stamps_finished_at() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        c.task_repo.mark_finished(t.id, h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        let r = host_row(&got, h);
        assert_eq!(r.state, TaskState::Done);
        assert!(r.finished_at.is_some());
    }

    #[tokio::test]
    async fn mark_failed_sets_the_host_row_failed_with_error_text_and_finished_at() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        c.task_repo.mark_failed(t.id, h, "disk exploded").await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        let r = host_row(&got, h);
        assert_eq!(r.state, TaskState::Failed);
        assert_eq!(r.error.as_deref(), Some("disk exploded"));
        assert!(r.finished_at.is_some());
    }

    #[tokio::test]
    async fn mark_finished_overwrites_a_failed_host_row_because_it_lacks_a_terminal_guard() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.mark_failed(t.id, h, "boom").await.unwrap();
        c.task_repo.mark_finished(t.id, h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        let r = host_row(&got, h);
        assert_eq!(r.state, TaskState::Done);
        assert_eq!(r.error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn mark_failed_overwrites_a_done_host_row_because_it_lacks_a_terminal_guard() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.mark_finished(t.id, h).await.unwrap();
        c.task_repo.mark_failed(t.id, h, "late failure").await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        let r = host_row(&got, h);
        assert_eq!(r.state, TaskState::Failed);
        assert_eq!(r.error.as_deref(), Some("late failure"));
    }

    #[tokio::test]
    async fn mark_all_finished_completes_pending_and_running_hosts() {
        let (c, _g) = container().await;
        let pending_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let running_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Multicast, vec![pending_h, running_h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, running_h).await.unwrap();
        c.task_repo.mark_all_finished(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, pending_h).state, TaskState::Done);
        assert_eq!(host_row(&got, running_h).state, TaskState::Done);
        assert!(host_row(&got, pending_h).finished_at.is_some());
        assert!(host_row(&got, running_h).finished_at.is_some());
        assert_eq!(got.aggregate_state(), TaskState::Done);
    }

    #[tokio::test]
    async fn mark_all_finished_leaves_done_and_failed_rows_untouched() {
        let (c, _g) = container().await;
        let done_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let failed_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Multicast, vec![done_h, failed_h], None)
            .await
            .unwrap();
        c.task_repo.mark_finished(t.id, done_h).await.unwrap();
        c.task_repo.mark_failed(t.id, failed_h, "boom").await.unwrap();
        c.task_repo.mark_all_finished(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, done_h).state, TaskState::Done);
        assert_eq!(host_row(&got, failed_h).state, TaskState::Failed);
        assert_eq!(host_row(&got, failed_h).error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn mark_all_finished_leaves_cancelled_rows_untouched() {
        let (c, _g) = container().await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Multicast, vec![h1, h2], None)
            .await
            .unwrap();
        c.task_repo.cancel(t.id).await.unwrap();
        c.task_repo.mark_all_finished(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, h1).state, TaskState::Cancelled);
        assert_eq!(host_row(&got, h2).state, TaskState::Cancelled);
    }

    #[tokio::test]
    async fn mark_all_failed_fails_pending_and_running_hosts_with_error() {
        let (c, _g) = container().await;
        let pending_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let running_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Multicast, vec![pending_h, running_h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, running_h).await.unwrap();
        c.task_repo.mark_all_failed(t.id, "net down").await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, pending_h).state, TaskState::Failed);
        assert_eq!(host_row(&got, running_h).state, TaskState::Failed);
        assert_eq!(host_row(&got, pending_h).error.as_deref(), Some("net down"));
        assert!(host_row(&got, running_h).finished_at.is_some());
        assert_eq!(got.aggregate_state(), TaskState::Failed);
    }

    #[tokio::test]
    async fn mark_all_failed_leaves_done_rows_untouched() {
        let (c, _g) = container().await;
        let done_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let pending_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Multicast, vec![done_h, pending_h], None)
            .await
            .unwrap();
        c.task_repo.mark_finished(t.id, done_h).await.unwrap();
        c.task_repo.mark_all_failed(t.id, "net down").await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, done_h).state, TaskState::Done);
        assert!(host_row(&got, done_h).error.is_none());
        assert_eq!(host_row(&got, pending_h).state, TaskState::Failed);
    }

    #[tokio::test]
    async fn cancel_flips_pending_and_running_hosts_to_cancelled() {
        let (c, _g) = container().await;
        let pending_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let running_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![pending_h, running_h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, running_h).await.unwrap();
        c.task_repo.cancel(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, pending_h).state, TaskState::Cancelled);
        assert_eq!(host_row(&got, running_h).state, TaskState::Cancelled);
        assert_eq!(got.aggregate_state(), TaskState::Cancelled);
    }

    #[tokio::test]
    async fn cancel_leaves_done_hosts_untouched_and_yields_partial_aggregate() {
        let (c, _g) = container().await;
        let done_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let pending_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![done_h, pending_h], None)
            .await
            .unwrap();
        c.task_repo.mark_finished(t.id, done_h).await.unwrap();
        c.task_repo.cancel(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, done_h).state, TaskState::Done);
        assert_eq!(host_row(&got, pending_h).state, TaskState::Cancelled);
        assert_eq!(got.aggregate_state(), TaskState::Partial);
    }

    #[tokio::test]
    async fn cancel_leaves_failed_hosts_untouched_and_yields_failed_aggregate() {
        let (c, _g) = container().await;
        let failed_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let pending_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![failed_h, pending_h], None)
            .await
            .unwrap();
        c.task_repo.mark_failed(t.id, failed_h, "boom").await.unwrap();
        c.task_repo.cancel(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, failed_h).state, TaskState::Failed);
        assert_eq!(host_row(&got, pending_h).state, TaskState::Cancelled);
        assert_eq!(got.aggregate_state(), TaskState::Failed);
    }

    #[tokio::test]
    async fn retry_does_not_touch_pending_or_running_host_rows() {
        let (c, _g) = container().await;
        let pending_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let running_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![pending_h, running_h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, running_h).await.unwrap();
        c.task_repo.retry(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, pending_h).state, TaskState::Pending);
        assert_eq!(host_row(&got, running_h).state, TaskState::Running);
    }

    #[tokio::test]
    async fn retry_resets_failed_and_cancelled_hosts_to_pending_leaving_done_alone() {
        let (c, _g) = container().await;
        let done_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let failed_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let cancelled_h = host(&c, "aa:bb:cc:dd:ee:03").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![done_h, failed_h, cancelled_h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, failed_h).await.unwrap();
        c.task_repo.mark_finished(t.id, done_h).await.unwrap();
        c.task_repo.mark_failed(t.id, failed_h, "boom").await.unwrap();
        c.task_repo.cancel(t.id).await.unwrap();

        let before = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&before, cancelled_h).state, TaskState::Cancelled);
        assert_eq!(before.aggregate_state(), TaskState::Partial);

        c.task_repo.retry(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, done_h).state, TaskState::Done);
        assert_eq!(host_row(&got, failed_h).state, TaskState::Pending);
        assert_eq!(host_row(&got, cancelled_h).state, TaskState::Pending);
        let reset = host_row(&got, failed_h);
        assert!(reset.error.is_none());
        assert!(reset.started_at.is_none());
        assert!(reset.finished_at.is_none());
        assert_eq!(got.aggregate_state(), TaskState::Running);
    }

    #[tokio::test]
    async fn get_active_by_image_returns_only_active_tasks_for_that_image_newest_first() {
        let (c, _g) = container().await;
        let img_a = image(&c, "a").await;
        let img_b = image(&c, "b").await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        let h3 = host(&c, "aa:bb:cc:dd:ee:03").await;

        let t1 = c
            .task_repo
            .create(TaskType::Deploy, vec![h1], Some(img_a))
            .await
            .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let t2 = c
            .task_repo
            .create(TaskType::Deploy, vec![h2], Some(img_a))
            .await
            .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let terminal = c
            .task_repo
            .create(TaskType::Deploy, vec![h3], Some(img_a))
            .await
            .unwrap();
        c.task_repo.mark_finished(terminal.id, h3).await.unwrap();
        let other = c
            .task_repo
            .create(TaskType::Deploy, vec![h1], Some(img_b))
            .await
            .unwrap();

        c.task_repo.start(t2.id, h2).await.unwrap();

        let active = c.task_repo.get_active_by_image(img_a).await.unwrap();
        let ids: Vec<i64> = active.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![t2.id, t1.id]);
        assert!(!ids.contains(&terminal.id));
        assert!(!ids.contains(&other.id));
    }

    #[tokio::test]
    async fn get_next_multicast_returns_a_multicast_task_with_a_pending_host_only() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;

        c.task_repo
            .create(TaskType::Deploy, vec![h], None)
            .await
            .unwrap();
        assert!(c.task_repo.get_next_multicast().await.unwrap().is_none());

        let m = c
            .task_repo
            .create(TaskType::Multicast, vec![h], None)
            .await
            .unwrap();
        let got = c.task_repo.get_next_multicast().await.unwrap().unwrap();
        assert_eq!(got.id, m.id);
        assert_eq!(got.task_type, TaskType::Multicast);

        c.task_repo.start(m.id, h).await.unwrap();
        assert!(c.task_repo.get_next_multicast().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_next_multicast_includes_a_task_while_any_host_is_still_pending() {
        let (c, _g) = container().await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        let m = c
            .task_repo
            .create(TaskType::Multicast, vec![h1, h2], None)
            .await
            .unwrap();
        c.task_repo.start(m.id, h1).await.unwrap();
        assert_eq!(
            c.task_repo.get_next_multicast().await.unwrap().unwrap().id,
            m.id
        );

        c.task_repo.start(m.id, h2).await.unwrap();
        c.task_repo.mark_finished(m.id, h1).await.unwrap();
        c.task_repo.mark_finished(m.id, h2).await.unwrap();
        assert!(c.task_repo.get_next_multicast().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn deleting_a_host_cascades_away_its_task_host_rows() {
        let (c, _g) = container().await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![h1, h2], None)
            .await
            .unwrap();
        c.host_repo.delete(h1).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(got.hosts.len(), 1);
        assert_eq!(got.hosts[0].host_id, h2);
        assert_eq!(got.aggregate_state(), TaskState::Pending);
    }

    #[tokio::test]
    async fn deleting_the_last_host_leaves_a_defunct_task_that_aggregates_to_cancelled() {
        let (c, _g) = container().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![h], None)
            .await
            .unwrap();
        c.host_repo.delete(h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert!(got.hosts.is_empty());
        assert_eq!(got.aggregate_state(), TaskState::Cancelled);
    }

    #[tokio::test]
    #[ignore = "issues.md: removing an image does not cancel tasks that reference it"]
    async fn removing_an_image_cancels_tasks_that_reference_it() {
        let (c, _g) = container().await;
        let img = image(&c, "doomed").await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![h], Some(img))
            .await
            .unwrap();
        c.image_repo.delete_image(img).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(got.aggregate_state(), TaskState::Cancelled);
        assert!(c.task_repo.get_active_by_image(img).await.unwrap().is_empty());
    }
}
