use crate::domain::task::{
    MAX_TASK_PAGE_SIZE, SCRIPT_OUTPUT_HEAD_BYTES, SCRIPT_OUTPUT_TAIL_BYTES, ScriptRun, Task,
    TaskHost, TaskPage, TaskQuery, TaskRepository, TaskType,
};
use imaged_model::error::{AppError, Result};
use imaged_model::task::TaskState;
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use derive_more::Constructor;
use imaged_model::script::InputValues;
use sqlx::SqlitePool;
use sqlx::types::Json;

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
    type Error = imaged_model::error::AppError;

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
        self.create_with_inputs(
            task_type,
            host_ids,
            image_id,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .await
    }

    async fn create_with_inputs(
        &self,
        task_type: TaskType,
        host_ids: Vec<i64>,
        image_id: Option<i64>,
        inputs: &BTreeMap<i64, InputValues>,
        partuuids: &BTreeMap<i64, String>,
    ) -> Result<Task> {
        let type_str = task_type.to_string();
        let pending_str = TaskState::Pending.to_string();
        let now = Utc::now();

        let mut tx = self.pool.begin().await?;

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
        .fetch_one(&mut *tx)
        .await?;

        for host_id in host_ids.iter() {
            let partuuid = partuuids.get(host_id);
            sqlx::query!(
                "INSERT INTO task_hosts (task_id, host_id, state, partuuid) VALUES (?, ?, ?, ?)",
                task.id,
                host_id,
                pending_str,
                partuuid
            )
            .execute(&mut *tx)
            .await?;
            for (name, value) in inputs.get(host_id).into_iter().flatten() {
                sqlx::query!(
                    "INSERT INTO task_host_inputs (task_id, host_id, name, value) VALUES (?, ?, ?, ?)",
                    task.id,
                    host_id,
                    name,
                    value
                )
                .execute(&mut *tx)
                .await?;
            }
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
        let pending = TaskState::Pending.to_string();
        let running = TaskState::Running.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE task_hosts SET state = ?, finished_at = ? \
             WHERE task_id = ? AND host_id = ? AND (state = ? OR state = ?)",
            state,
            now,
            task_id,
            host_id,
            pending,
            running
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_failed(&self, task_id: i64, host_id: i64, error: &str) -> Result {
        let state = TaskState::Failed.to_string();
        let pending = TaskState::Pending.to_string();
        let running = TaskState::Running.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE task_hosts SET state = ?, finished_at = ?, error = ? \
             WHERE task_id = ? AND host_id = ? AND (state = ? OR state = ?)",
            state,
            now,
            error,
            task_id,
            host_id,
            pending,
            running
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn fail_running(&self, task_type: TaskType, error: &str) -> Result {
        let failed = TaskState::Failed.to_string();
        let running = TaskState::Running.to_string();
        let task_type = task_type.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE task_hosts SET state = ?, finished_at = ?, error = ? \
             WHERE state = ? AND task_id IN (SELECT id FROM tasks WHERE type = ?)",
            failed,
            now,
            error,
            running,
            task_type
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn retry(&self, id: i64) -> Result {
        let pending = TaskState::Pending.to_string();
        let failed = TaskState::Failed.to_string();
        let cancelled = TaskState::Cancelled.to_string();

        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "DELETE FROM script_runs WHERE task_id = ? AND host_id IN \
             (SELECT host_id FROM task_hosts WHERE task_id = ? AND (state = ? OR state = ?))",
            id,
            id,
            failed,
            cancelled
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE task_hosts SET state = ?, error = NULL, started_at = NULL, finished_at = NULL \
             WHERE task_id = ? AND (state = ? OR state = ?)",
            pending,
            id,
            failed,
            cancelled
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

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

    async fn list(&self, query: &TaskQuery) -> Result<TaskPage> {
        if !(1..=MAX_TASK_PAGE_SIZE).contains(&query.limit) {
            return Err(AppError::InvalidArgument(format!(
                "limit must be between 1 and {MAX_TASK_PAGE_SIZE}"
            )));
        }
        let states = Json(&query.states);
        let task_types: Vec<String> = query.task_types.iter().map(|t| t.to_string()).collect();
        let task_types = Json(&task_types);
        let fetched = i64::from(query.limit) + 1;
        let rows = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT
                twh.id as "id!", twh.type as task_type, twh.image_id, twh.image_name,
                twh.image_deleted as "image_deleted!: i64",
                twh.created_at as "created_at: DateTime<Utc>",
                twh.hosts as "hosts!: sqlx::types::Json<Vec<TaskHost>>"
            FROM tasks_with_hosts twh
            WHERE (?1 IS NULL OR twh.id < ?1)
            AND (json_array_length(?2) = 0 OR twh.state IN (SELECT value FROM json_each(?2)))
            AND (json_array_length(?3) = 0 OR twh.type IN (SELECT value FROM json_each(?3)))
            AND (?4 IS NULL OR EXISTS (
                SELECT 1 FROM task_hosts th WHERE th.task_id = twh.id AND th.host_id = ?4
            ))
            ORDER BY twh.id DESC
            LIMIT ?5
            "#,
            query.before,
            states,
            task_types,
            query.host_id,
            fetched
        )
        .fetch_all(&self.pool)
        .await?;

        let mut tasks = rows
            .into_iter()
            .map(Task::try_from)
            .collect::<Result<Vec<_>>>()?;
        let limit = query.limit as usize;
        let next_cursor = if tasks.len() > limit {
            tasks.truncate(limit);
            tasks.last().map(|task| task.id)
        } else {
            None
        };
        Ok(TaskPage { tasks, next_cursor })
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
        let multicast = TaskType::Multicast.to_string();
        let multicast_partition = TaskType::MulticastPartition.to_string();
        let row = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT
                twh.id as "id!", twh.type as task_type, twh.image_id, twh.image_name,
                twh.image_deleted as "image_deleted!: i64",
                twh.created_at as "created_at: DateTime<Utc>",
                twh.hosts as "hosts!: sqlx::types::Json<Vec<TaskHost>>"
            FROM tasks_with_hosts twh
            WHERE (twh.type = ? OR twh.type = ?)
            AND EXISTS (
                SELECT 1 FROM task_hosts th
                WHERE th.task_id = twh.id AND th.state = ?
            )
            ORDER BY twh.id ASC
            LIMIT 1
            "#,
            multicast,
            multicast_partition,
            pending_state
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.try_into()).transpose()?)
    }

    async fn get_inputs(&self, task_id: i64, host_id: i64) -> Result<InputValues> {
        Ok(sqlx::query!(
            "SELECT name, value FROM task_host_inputs WHERE task_id = ? AND host_id = ?",
            task_id,
            host_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| (row.name, row.value))
        .collect())
    }

    async fn start_script_run(
        &self,
        task_id: i64,
        host_id: i64,
        position: i64,
        script_name: &str,
    ) -> Result {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "DELETE FROM script_runs WHERE task_id = ? AND host_id = ? AND position >= ?",
            task_id,
            host_id,
            position
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "INSERT INTO script_runs (task_id, host_id, position, script_name, started_at) \
             VALUES (?, ?, ?, ?, ?)",
            task_id,
            host_id,
            position,
            script_name,
            now
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn append_script_output(
        &self,
        task_id: i64,
        host_id: i64,
        position: i64,
        output: &str,
    ) -> Result {
        let length = output.len() as i64;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let total = sqlx::query_scalar!(
            "UPDATE script_runs SET output_bytes = output_bytes + ? \
             WHERE task_id = ? AND host_id = ? AND position = ? AND finished_at IS NULL \
             RETURNING output_bytes",
            length,
            task_id,
            host_id,
            position
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| no_running_script(task_id, host_id, position))?;

        for (start, text) in kept_output(total - length, output) {
            sqlx::query!(
                "INSERT INTO script_run_output (task_id, host_id, position, start, text) \
                 VALUES (?, ?, ?, ?, ?)",
                task_id,
                host_id,
                position,
                start,
                text
            )
            .execute(&mut *tx)
            .await?;
        }

        let head_end = SCRIPT_OUTPUT_HEAD_BYTES as i64;
        let tail_start = total - SCRIPT_OUTPUT_TAIL_BYTES as i64;
        sqlx::query!(
            "DELETE FROM script_run_output \
             WHERE task_id = ? AND host_id = ? AND position = ? \
             AND start >= ? AND start + length(CAST(text AS BLOB)) <= ?",
            task_id,
            host_id,
            position,
            head_end,
            tail_start
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn finish_script_run(
        &self,
        task_id: i64,
        host_id: i64,
        position: i64,
        exit_code: Option<i64>,
    ) -> Result {
        let now = Utc::now();
        let updated = sqlx::query!(
            "UPDATE script_runs SET exit_code = ?, finished_at = ? \
             WHERE task_id = ? AND host_id = ? AND position = ? AND finished_at IS NULL",
            exit_code,
            now,
            task_id,
            host_id,
            position
        )
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(no_running_script(task_id, host_id, position));
        }
        Ok(())
    }

    async fn get_script_runs(&self, task_id: i64, host_id: i64) -> Result<Vec<ScriptRun>> {
        let mut chunks: BTreeMap<i64, Vec<(i64, String)>> = BTreeMap::new();
        for chunk in sqlx::query!(
            "SELECT position, start, text FROM script_run_output \
             WHERE task_id = ? AND host_id = ? ORDER BY position, start",
            task_id,
            host_id
        )
        .fetch_all(&self.pool)
        .await?
        {
            chunks
                .entry(chunk.position)
                .or_default()
                .push((chunk.start, chunk.text));
        }

        Ok(sqlx::query!(
            r#"SELECT
                 position,
                 script_name,
                 exit_code,
                 started_at as "started_at: DateTime<Utc>",
                 finished_at as "finished_at: DateTime<Utc>"
               FROM script_runs
               WHERE task_id = ? AND host_id = ?
               ORDER BY position"#,
            task_id,
            host_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|run| ScriptRun {
            output: assemble_output(&chunks.remove(&run.position).unwrap_or_default()),
            position: run.position,
            script_name: run.script_name,
            exit_code: run.exit_code,
            started_at: run.started_at,
            finished_at: run.finished_at,
        })
        .collect())
    }
}

fn kept_output(start: i64, text: &str) -> Vec<(i64, &str)> {
    let mut kept = Vec::with_capacity(2);
    let mut rest = text;
    let mut rest_start = start;
    let head_room = SCRIPT_OUTPUT_HEAD_BYTES as i64 - start;
    if head_room > 0 {
        let cut = ceil_char_boundary(text, head_room as usize);
        kept.push((start, &text[..cut]));
        rest = &text[cut..];
        rest_start += cut as i64;
    }
    if !rest.is_empty() {
        let skip = ceil_char_boundary(rest, rest.len().saturating_sub(SCRIPT_OUTPUT_TAIL_BYTES));
        kept.push((rest_start + skip as i64, &rest[skip..]));
    }
    kept
}

fn assemble_output(chunks: &[(i64, String)]) -> String {
    let head_end = SCRIPT_OUTPUT_HEAD_BYTES as i64;
    let (head, tail): (Vec<_>, Vec<_>) = chunks.iter().partition(|(start, _)| *start < head_end);
    let mut output: String = head.iter().map(|(_, text)| text.as_str()).collect();
    let Some((tail_start, _)) = tail.first() else {
        return output;
    };
    let tail: String = tail.iter().map(|(_, text)| text.as_str()).collect();
    let skip = ceil_char_boundary(&tail, tail.len().saturating_sub(SCRIPT_OUTPUT_TAIL_BYTES));
    let omitted = tail_start + skip as i64 - output.len() as i64;
    if omitted > 0 {
        output.push_str(&format!("\n[… {omitted} bytes of output omitted …]\n"));
    }
    output.push_str(&tail[skip..]);
    output
}

fn ceil_char_boundary(text: &str, index: usize) -> usize {
    (index.min(text.len())..=text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len())
}

fn no_running_script(task_id: i64, host_id: i64, position: i64) -> AppError {
    AppError::InvalidArgument(format!(
        "script {position} of task {task_id} is not running on host {host_id}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Repos, repos};
    use imaged_model::error::AppError;
    use imaged_model::image::ImageKind;
    use std::time::Duration;

    async fn host(c: &Repos, mac: &str) -> i64 {
        c.host_repo
            .upsert_host(mac.into(), Ok(1_000_000), None)
            .await
            .unwrap()
            .id
    }

    async fn image(c: &Repos, name: &str) -> i64 {
        c.image_repo
            .create_image(name.into(), ImageKind::Disk)
            .await
            .unwrap()
            .id
    }

    fn host_row(task: &Task, host_id: i64) -> &TaskHost {
        task.hosts
            .iter()
            .find(|h| h.host_id == host_id)
            .expect("host row present on task")
    }

    fn query(limit: u32) -> TaskQuery {
        TaskQuery {
            states: Vec::new(),
            task_types: Vec::new(),
            host_id: None,
            before: None,
            limit,
        }
    }

    async fn listed(c: &Repos, query: TaskQuery) -> Vec<i64> {
        let page = c.task_repo.list(&query).await.unwrap();
        page.tasks.iter().map(|t| t.id).collect()
    }

    #[tokio::test]
    async fn create_inserts_a_pending_row_for_each_host_and_returns_them_attached() {
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
        let err = c
            .task_repo
            .create(TaskType::Reboot, vec![999_999], None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::FailedPrecondition(_)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn create_with_a_nonexistent_host_id_leaves_no_orphaned_task_row_behind() {
        let (c, _g) = repos().await;
        let _ = c
            .task_repo
            .create(TaskType::Reboot, vec![999_999], None)
            .await;
        let all = listed(&c, query(MAX_TASK_PAGE_SIZE)).await;
        assert!(
            all.is_empty(),
            "a failed create must not leave a task row behind"
        );
    }

    #[tokio::test]
    async fn get_returns_the_task_with_its_host_rows() {
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
        let err = c.task_repo.get(999_999).await.unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn list_returns_tasks_newest_first_with_named_host_rows_grouped_per_task() {
        let (c, _g) = repos().await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        c.host_repo.update_name(h2, "lab-2".into()).await.unwrap();

        let t1 = c
            .task_repo
            .create(TaskType::Reboot, vec![h1], None)
            .await
            .unwrap();
        let t2 = c
            .task_repo
            .create(TaskType::Deploy, vec![h1, h2], None)
            .await
            .unwrap();
        let t3 = c
            .task_repo
            .create(TaskType::Reboot, vec![h2], None)
            .await
            .unwrap();

        let page = c.task_repo.list(&query(10)).await.unwrap();
        let ids: Vec<i64> = page.tasks.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![t3.id, t2.id, t1.id]);
        assert_eq!(page.next_cursor, None);

        let middle = page.tasks.iter().find(|t| t.id == t2.id).unwrap();
        let mut middle_hosts: Vec<(i64, &str)> = middle
            .hosts
            .iter()
            .map(|h| (h.host_id, h.host_name.as_str()))
            .collect();
        middle_hosts.sort();
        assert_eq!(middle_hosts, vec![(h1, "aa-bb-cc-dd-ee-01"), (h2, "lab-2")]);
        assert_eq!(host_row(&page.tasks[0], h2).mac, "aa:bb:cc:dd:ee:02");
    }

    #[tokio::test]
    async fn list_pages_through_every_task_with_the_returned_cursor() {
        let (c, _g) = repos().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let mut created = Vec::new();
        for _ in 0..5 {
            let task = c
                .task_repo
                .create(TaskType::Reboot, vec![h], None)
                .await
                .unwrap();
            created.push(task.id);
        }
        created.reverse();

        let mut pages = Vec::new();
        let mut before = None;
        loop {
            let page = c
                .task_repo
                .list(&TaskQuery { before, ..query(2) })
                .await
                .unwrap();
            pages.push(page.tasks.iter().map(|t| t.id).collect::<Vec<_>>());
            match page.next_cursor {
                Some(cursor) => before = Some(cursor),
                None => break,
            }
        }

        assert_eq!(
            pages,
            [
                created[0..2].to_vec(),
                created[2..4].to_vec(),
                created[4..].to_vec()
            ]
        );
    }

    #[tokio::test]
    async fn a_page_that_ends_exactly_at_the_last_task_has_no_cursor() {
        let (c, _g) = repos().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        for _ in 0..2 {
            c.task_repo
                .create(TaskType::Reboot, vec![h], None)
                .await
                .unwrap();
        }

        let page = c.task_repo.list(&query(2)).await.unwrap();

        assert_eq!(page.tasks.len(), 2);
        assert_eq!(page.next_cursor, None);
    }

    #[tokio::test]
    async fn list_rejects_a_page_size_outside_the_allowed_range() {
        let (c, _g) = repos().await;
        for limit in [0, MAX_TASK_PAGE_SIZE + 1] {
            let err = c.task_repo.list(&query(limit)).await.unwrap_err();
            assert!(matches!(err, AppError::InvalidArgument(_)), "got {err:?}");
        }
    }

    #[tokio::test]
    async fn list_filters_on_the_same_aggregate_state_the_task_reports() {
        let (c, _g) = repos().await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        let deploy = |hosts: Vec<i64>| {
            let task_repo = c.task_repo.clone();
            async move {
                task_repo
                    .create(TaskType::Deploy, hosts, None)
                    .await
                    .unwrap()
                    .id
            }
        };

        deploy(vec![]).await;
        deploy(vec![h1, h2]).await;
        let running = deploy(vec![h1, h2]).await;
        c.task_repo.start(running, h1).await.unwrap();
        let half_done = deploy(vec![h1, h2]).await;
        c.task_repo.mark_finished(half_done, h1).await.unwrap();
        let done = deploy(vec![h1]).await;
        c.task_repo.mark_finished(done, h1).await.unwrap();
        let failed = deploy(vec![h1, h2]).await;
        c.task_repo.mark_failed(failed, h1, "x").await.unwrap();
        c.task_repo.cancel(failed).await.unwrap();
        let cancelled = deploy(vec![h1, h2]).await;
        c.task_repo.cancel(cancelled).await.unwrap();
        let partial = deploy(vec![h1, h2]).await;
        c.task_repo.mark_finished(partial, h1).await.unwrap();
        c.task_repo.mark_failed(partial, h2, "x").await.unwrap();
        let partly_cancelled = deploy(vec![h1, h2]).await;
        c.task_repo
            .mark_finished(partly_cancelled, h1)
            .await
            .unwrap();
        c.task_repo.cancel(partly_cancelled).await.unwrap();

        let every = c
            .task_repo
            .list(&query(MAX_TASK_PAGE_SIZE))
            .await
            .unwrap()
            .tasks;
        for states in [
            vec![TaskState::Pending],
            vec![TaskState::Running],
            vec![TaskState::Done],
            vec![TaskState::Failed],
            vec![TaskState::Cancelled],
            vec![TaskState::Partial],
            vec![TaskState::Failed, TaskState::Partial],
        ] {
            let expected: Vec<i64> = every
                .iter()
                .filter(|t| states.contains(&t.aggregate_state()))
                .map(|t| t.id)
                .collect();
            assert!(!expected.is_empty(), "{states:?}");
            let got = listed(
                &c,
                TaskQuery {
                    states: states.clone(),
                    ..query(MAX_TASK_PAGE_SIZE)
                },
            )
            .await;
            assert_eq!(got, expected, "{states:?}");
        }
    }

    #[tokio::test]
    async fn list_filters_by_task_type_and_host() {
        let (c, _g) = repos().await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        let reboot = c
            .task_repo
            .create(TaskType::Reboot, vec![h1], None)
            .await
            .unwrap();
        let deploy = c
            .task_repo
            .create(TaskType::Deploy, vec![h1, h2], None)
            .await
            .unwrap();
        let other_reboot = c
            .task_repo
            .create(TaskType::Reboot, vec![h2], None)
            .await
            .unwrap();

        let reboots = TaskQuery {
            task_types: vec![TaskType::Reboot],
            ..query(10)
        };
        assert_eq!(
            listed(&c, reboots.clone()).await,
            [other_reboot.id, reboot.id]
        );
        assert_eq!(
            listed(
                &c,
                TaskQuery {
                    host_id: Some(h1),
                    ..query(10)
                }
            )
            .await,
            [deploy.id, reboot.id]
        );
        assert_eq!(
            listed(
                &c,
                TaskQuery {
                    host_id: Some(h2),
                    ..reboots
                }
            )
            .await,
            [other_reboot.id]
        );
    }

    #[tokio::test]
    async fn fail_running_fails_only_running_rows_of_that_task_type() {
        let (c, _g) = repos().await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        let multicast = c
            .task_repo
            .create(TaskType::Multicast, vec![h1, h2], None)
            .await
            .unwrap();
        c.task_repo.start(multicast.id, h1).await.unwrap();
        let deploy = c
            .task_repo
            .create(TaskType::Deploy, vec![h2], None)
            .await
            .unwrap();
        c.task_repo.start(deploy.id, h2).await.unwrap();

        c.task_repo
            .fail_running(TaskType::Multicast, "server stopped")
            .await
            .unwrap();

        let multicast = c.task_repo.get(multicast.id).await.unwrap();
        let orphan = host_row(&multicast, h1);
        assert_eq!(orphan.state, TaskState::Failed);
        assert_eq!(orphan.error.as_deref(), Some("server stopped"));
        assert!(orphan.finished_at.is_some());
        assert_eq!(host_row(&multicast, h2).state, TaskState::Pending);
        let deploy = c.task_repo.get(deploy.id).await.unwrap();
        assert_eq!(host_row(&deploy, h2).state, TaskState::Running);
    }

    #[tokio::test]
    async fn get_next_prefers_running_over_pending_then_oldest_created_first() {
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
        let h = host(&c, "aa:bb:cc:dd:ee:09").await;
        assert!(c.task_repo.get_next(h).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn start_transitions_a_pending_host_row_to_running_and_stamps_started_at() {
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.start(t.id, h).await.unwrap();
        c.task_repo
            .mark_failed(t.id, h, "disk exploded")
            .await
            .unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        let r = host_row(&got, h);
        assert_eq!(r.state, TaskState::Failed);
        assert_eq!(r.error.as_deref(), Some("disk exploded"));
        assert!(r.finished_at.is_some());
    }

    #[tokio::test]
    async fn mark_finished_does_not_resurrect_a_failed_host_row() {
        let (c, _g) = repos().await;
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
        assert_eq!(r.state, TaskState::Failed);
        assert_eq!(r.error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn mark_finished_does_not_resurrect_a_cancelled_host_row() {
        let (c, _g) = repos().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.cancel(t.id).await.unwrap();
        c.task_repo.mark_finished(t.id, h).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, h).state, TaskState::Cancelled);
    }

    #[tokio::test]
    async fn mark_failed_does_not_resurrect_a_done_host_row() {
        let (c, _g) = repos().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.mark_finished(t.id, h).await.unwrap();
        c.task_repo
            .mark_failed(t.id, h, "late failure")
            .await
            .unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        let r = host_row(&got, h);
        assert_eq!(r.state, TaskState::Done);
        assert!(r.error.is_none());
    }

    #[tokio::test]
    async fn mark_failed_does_not_resurrect_a_cancelled_host_row() {
        let (c, _g) = repos().await;
        let h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let t = c
            .task_repo
            .create(TaskType::Reboot, vec![h], None)
            .await
            .unwrap();
        c.task_repo.cancel(t.id).await.unwrap();
        c.task_repo
            .mark_failed(t.id, h, "late failure")
            .await
            .unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        let r = host_row(&got, h);
        assert_eq!(r.state, TaskState::Cancelled);
        assert!(r.error.is_none());
    }

    #[tokio::test]
    async fn cancel_flips_pending_and_running_hosts_to_cancelled() {
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
        let failed_h = host(&c, "aa:bb:cc:dd:ee:01").await;
        let pending_h = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create(TaskType::Deploy, vec![failed_h, pending_h], None)
            .await
            .unwrap();
        c.task_repo
            .mark_failed(t.id, failed_h, "boom")
            .await
            .unwrap();
        c.task_repo.cancel(t.id).await.unwrap();
        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(host_row(&got, failed_h).state, TaskState::Failed);
        assert_eq!(host_row(&got, pending_h).state, TaskState::Cancelled);
        assert_eq!(got.aggregate_state(), TaskState::Failed);
    }

    #[tokio::test]
    async fn retry_does_not_touch_pending_or_running_host_rows() {
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        c.task_repo
            .mark_failed(t.id, failed_h, "boom")
            .await
            .unwrap();
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
    async fn each_host_keeps_its_target_partition_through_a_retry() {
        let (c, _g) = repos().await;
        let img = image(&c, "root").await;
        let h1 = host(&c, "aa:bb:cc:dd:ee:01").await;
        let h2 = host(&c, "aa:bb:cc:dd:ee:02").await;
        let t = c
            .task_repo
            .create_with_inputs(
                TaskType::MulticastPartition,
                vec![h1, h2],
                Some(img),
                &BTreeMap::new(),
                &BTreeMap::from([(h1, "part-a".to_string()), (h2, "part-b".to_string())]),
            )
            .await
            .unwrap();
        assert_eq!(
            c.task_repo
                .get_next_multicast()
                .await
                .unwrap()
                .map(|t| t.id),
            Some(t.id)
        );
        c.task_repo.start(t.id, h1).await.unwrap();
        c.task_repo.mark_failed(t.id, h1, "boom").await.unwrap();
        c.task_repo.retry(t.id).await.unwrap();

        let got = c.task_repo.get(t.id).await.unwrap();
        assert_eq!(got.task_type, TaskType::MulticastPartition);
        assert_eq!(got.partuuid(h1), Some("part-a"));
        assert_eq!(got.partuuid(h2), Some("part-b"));
        let multicasts = TaskQuery {
            task_types: vec![TaskType::Multicast, TaskType::MulticastPartition],
            ..query(10)
        };
        assert_eq!(listed(&c, multicasts).await, [t.id]);
    }

    #[tokio::test]
    async fn get_active_by_image_returns_only_active_tasks_for_that_image_newest_first() {
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
    async fn retrying_forgets_the_script_runs_of_the_hosts_it_resets() {
        let (c, _g) = repos().await;
        let img = image(&c, "img").await;
        let failed = host(&c, "aa:bb:cc:dd:ee:01").await;
        let done = host(&c, "aa:bb:cc:dd:ee:02").await;
        let task = c
            .task_repo
            .create(TaskType::Multicast, vec![failed, done], Some(img))
            .await
            .unwrap();
        for host_id in [failed, done] {
            c.task_repo.start(task.id, host_id).await.unwrap();
            c.task_repo
                .start_script_run(task.id, host_id, 0, "setup")
                .await
                .unwrap();
            c.task_repo
                .append_script_output(task.id, host_id, 0, "hello\n")
                .await
                .unwrap();
            c.task_repo
                .finish_script_run(task.id, host_id, 0, Some(1))
                .await
                .unwrap();
        }
        c.task_repo.mark_failed(task.id, failed, "x").await.unwrap();
        c.task_repo.mark_finished(task.id, done).await.unwrap();

        c.task_repo.retry(task.id).await.unwrap();

        assert!(
            c.task_repo
                .get_script_runs(task.id, failed)
                .await
                .unwrap()
                .is_empty()
        );
        let kept = c.task_repo.get_script_runs(task.id, done).await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].output, "hello\n");
        assert_eq!(kept[0].exit_code, Some(1));
        assert!(kept[0].finished_at.is_some());
    }

    async fn running_script(c: &Repos, mac: &str, task_type: TaskType) -> (i64, i64) {
        let h = host(c, mac).await;
        let task = c.task_repo.create(task_type, vec![h], None).await.unwrap();
        c.task_repo.start(task.id, h).await.unwrap();
        c.task_repo
            .start_script_run(task.id, h, 0, "setup")
            .await
            .unwrap();
        (task.id, h)
    }

    async fn only_run(c: &Repos, task_id: i64, host_id: i64) -> ScriptRun {
        let mut runs = c.task_repo.get_script_runs(task_id, host_id).await.unwrap();
        assert_eq!(runs.len(), 1);
        runs.remove(0)
    }

    async fn stored_output_bytes(c: &Repos) -> i64 {
        sqlx::query_scalar(
            "SELECT COALESCE(SUM(length(CAST(text AS BLOB))), 0) FROM script_run_output",
        )
        .fetch_one(&c.pool)
        .await
        .unwrap()
    }

    fn omitted(bytes: usize) -> String {
        format!("\n[… {bytes} bytes of output omitted …]\n")
    }

    #[tokio::test]
    async fn output_up_to_the_cap_is_kept_verbatim_and_one_byte_more_is_marked_omitted() {
        let (c, _g) = repos().await;
        let (task, h) = running_script(&c, "aa:bb:cc:dd:ee:01", TaskType::Deploy).await;
        let line = format!("{}\n", "-".repeat(999));
        let mut written = String::new();
        while written.len() + line.len() <= SCRIPT_OUTPUT_HEAD_BYTES + SCRIPT_OUTPUT_TAIL_BYTES {
            c.task_repo
                .append_script_output(task, h, 0, &line)
                .await
                .unwrap();
            written.push_str(&line);
        }
        let filler =
            "z".repeat(SCRIPT_OUTPUT_HEAD_BYTES + SCRIPT_OUTPUT_TAIL_BYTES - written.len());
        c.task_repo
            .append_script_output(task, h, 0, &filler)
            .await
            .unwrap();
        written.push_str(&filler);
        assert_eq!(only_run(&c, task, h).await.output, written);

        c.task_repo
            .append_script_output(task, h, 0, "!")
            .await
            .unwrap();

        let expected = format!(
            "{}{}{}!",
            &written[..SCRIPT_OUTPUT_HEAD_BYTES],
            omitted(1),
            &written[SCRIPT_OUTPUT_HEAD_BYTES + 1..]
        );
        assert_eq!(only_run(&c, task, h).await.output, expected);
    }

    #[tokio::test]
    async fn long_output_keeps_the_head_and_the_latest_tail_with_bounded_storage() {
        let (c, _g) = repos().await;
        let (task, h) = running_script(&c, "aa:bb:cc:dd:ee:01", TaskType::Deploy).await;
        let head = "a".repeat(SCRIPT_OUTPUT_HEAD_BYTES);
        c.task_repo
            .append_script_output(task, h, 0, &head)
            .await
            .unwrap();
        let chunk = "b".repeat(100_000);
        for _ in 0..10 {
            c.task_repo
                .append_script_output(task, h, 0, &chunk)
                .await
                .unwrap();
            assert!(
                stored_output_bytes(&c).await
                    <= (SCRIPT_OUTPUT_HEAD_BYTES + 2 * SCRIPT_OUTPUT_TAIL_BYTES) as i64
            );
        }
        c.task_repo
            .append_script_output(task, h, 0, "the end\n")
            .await
            .unwrap();

        let tail = format!("{}the end\n", "b".repeat(SCRIPT_OUTPUT_TAIL_BYTES - 8));
        let expected = format!(
            "{head}{}{tail}",
            omitted(1_000_000 + 8 - SCRIPT_OUTPUT_TAIL_BYTES)
        );
        assert_eq!(only_run(&c, task, h).await.output, expected);
    }

    #[tokio::test]
    async fn one_huge_flush_stores_only_the_head_and_the_tail() {
        let (c, _g) = repos().await;
        let (task, h) = running_script(&c, "aa:bb:cc:dd:ee:01", TaskType::Deploy).await;
        let flood = format!(
            "{}{}",
            "x".repeat(SCRIPT_OUTPUT_HEAD_BYTES),
            "y".repeat(3 * SCRIPT_OUTPUT_TAIL_BYTES)
        );

        c.task_repo
            .append_script_output(task, h, 0, &flood)
            .await
            .unwrap();

        assert_eq!(
            stored_output_bytes(&c).await,
            (SCRIPT_OUTPUT_HEAD_BYTES + SCRIPT_OUTPUT_TAIL_BYTES) as i64
        );
        let expected = format!(
            "{}{}{}",
            "x".repeat(SCRIPT_OUTPUT_HEAD_BYTES),
            omitted(2 * SCRIPT_OUTPUT_TAIL_BYTES),
            "y".repeat(SCRIPT_OUTPUT_TAIL_BYTES)
        );
        assert_eq!(only_run(&c, task, h).await.output, expected);
    }

    #[tokio::test]
    async fn the_cap_never_splits_a_multibyte_character() {
        let (c, _g) = repos().await;
        let (task, h) = running_script(&c, "aa:bb:cc:dd:ee:01", TaskType::Deploy).await;
        let text = format!("a{}", "é".repeat(300_000));

        c.task_repo
            .append_script_output(task, h, 0, &text)
            .await
            .unwrap();

        let head = format!("a{}", "é".repeat(SCRIPT_OUTPUT_HEAD_BYTES / 2));
        let tail = "é".repeat(SCRIPT_OUTPUT_TAIL_BYTES / 2);
        let expected = format!(
            "{head}{}{tail}",
            omitted(text.len() - head.len() - tail.len())
        );
        assert_eq!(only_run(&c, task, h).await.output, expected);
    }

    #[tokio::test]
    async fn a_host_row_leaving_running_closes_its_unfinished_script_run() {
        let (c, _g) = repos().await;
        let failed = running_script(&c, "aa:bb:cc:dd:ee:01", TaskType::Deploy).await;
        c.task_repo
            .mark_failed(failed.0, failed.1, "lost")
            .await
            .unwrap();
        let cancelled = running_script(&c, "aa:bb:cc:dd:ee:02", TaskType::Deploy).await;
        c.task_repo.cancel(cancelled.0).await.unwrap();
        let done = running_script(&c, "aa:bb:cc:dd:ee:03", TaskType::Deploy).await;
        c.task_repo.mark_finished(done.0, done.1).await.unwrap();
        let orphaned = running_script(&c, "aa:bb:cc:dd:ee:04", TaskType::Multicast).await;
        c.task_repo
            .fail_running(TaskType::Multicast, "server stopped")
            .await
            .unwrap();

        for (task, h) in [failed, cancelled, done, orphaned] {
            let run = only_run(&c, task, h).await;
            assert!(run.finished_at.is_some());
            assert_eq!(run.exit_code, None);
            let err = c
                .task_repo
                .append_script_output(task, h, 0, "late")
                .await
                .unwrap_err();
            assert!(matches!(err, AppError::InvalidArgument(_)), "got {err:?}");
        }
    }

    #[tokio::test]
    async fn a_finished_script_run_keeps_its_exit_code_when_the_host_row_ends() {
        let (c, _g) = repos().await;
        let (task, h) = running_script(&c, "aa:bb:cc:dd:ee:01", TaskType::Deploy).await;
        c.task_repo
            .finish_script_run(task, h, 0, Some(3))
            .await
            .unwrap();
        let finished = only_run(&c, task, h).await;

        c.task_repo
            .mark_failed(task, h, "script failed")
            .await
            .unwrap();

        assert_eq!(only_run(&c, task, h).await, finished);
    }

    #[tokio::test]
    async fn restarting_a_script_at_the_same_position_starts_with_empty_output() {
        let (c, _g) = repos().await;
        let (task, h) = running_script(&c, "aa:bb:cc:dd:ee:01", TaskType::Deploy).await;
        let flood = "x".repeat(SCRIPT_OUTPUT_HEAD_BYTES + SCRIPT_OUTPUT_TAIL_BYTES + 1);
        c.task_repo
            .append_script_output(task, h, 0, &flood)
            .await
            .unwrap();

        c.task_repo
            .start_script_run(task, h, 0, "setup")
            .await
            .unwrap();
        c.task_repo
            .append_script_output(task, h, 0, "second run\n")
            .await
            .unwrap();

        assert_eq!(only_run(&c, task, h).await.output, "second run\n");
    }
}
