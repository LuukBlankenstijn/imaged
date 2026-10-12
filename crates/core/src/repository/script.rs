use std::collections::HashMap;

use chrono::{DateTime, Utc};
use derive_more::Constructor;
use sqlx::{SqliteConnection, SqlitePool, types::Json};

use crate::domain::script::{InputKind, NewScript, Script, ScriptInput, ScriptRepository};
use crate::error::{AppError, Result};

#[derive(Debug, Constructor)]
pub struct SqliteScriptRepository {
    pool: SqlitePool,
}

struct ScriptRow {
    id: i64,
    name: String,
    body: String,
    updated_at: DateTime<Utc>,
}

struct InputRow {
    script_id: i64,
    name: String,
    label: String,
    kind: String,
    required: bool,
    default_value: Option<String>,
    per_machine: bool,
    choices: Json<Vec<String>>,
}

impl SqliteScriptRepository {
    async fn assemble(&self, rows: Vec<ScriptRow>) -> Result<Vec<Script>> {
        let ids = Json(rows.iter().map(|row| row.id).collect::<Vec<_>>());
        let mut inputs: HashMap<i64, Vec<ScriptInput>> = HashMap::new();
        for row in sqlx::query_as!(
            InputRow,
            r#"SELECT
                 script_id,
                 name,
                 label,
                 kind,
                 required as "required: bool",
                 default_value,
                 per_machine as "per_machine: bool",
                 choices as "choices: Json<Vec<String>>"
               FROM script_inputs
               WHERE script_id IN (SELECT value FROM json_each(?))
               ORDER BY script_id, position"#,
            ids
        )
        .fetch_all(&self.pool)
        .await?
        {
            inputs.entry(row.script_id).or_default().push(ScriptInput {
                name: row.name,
                label: row.label,
                kind: InputKind::from_string(row.kind)?,
                required: row.required,
                default_value: row.default_value,
                per_machine: row.per_machine,
                choices: row.choices.0,
            });
        }
        Ok(rows
            .into_iter()
            .map(|row| Script {
                inputs: inputs.remove(&row.id).unwrap_or_default(),
                id: row.id,
                name: row.name,
                body: row.body,
                updated_at: row.updated_at,
            })
            .collect())
    }
}

async fn insert_inputs(conn: &mut SqliteConnection, script_id: i64, script: &NewScript) -> Result {
    for (position, input) in script.inputs.iter().enumerate() {
        let position = position as i64;
        let kind = input.kind.to_string();
        let choices = Json(&input.choices);
        sqlx::query!(
            "INSERT INTO script_inputs \
             (script_id, position, name, label, kind, required, default_value, per_machine, choices) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            script_id,
            position,
            input.name,
            input.label,
            kind,
            input.required,
            input.default_value,
            input.per_machine,
            choices
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl ScriptRepository for SqliteScriptRepository {
    async fn get_all(&self) -> Result<Vec<Script>> {
        let rows = sqlx::query_as!(
            ScriptRow,
            r#"SELECT id as "id!", name, body, updated_at as "updated_at: DateTime<Utc>"
               FROM scripts ORDER BY name"#
        )
        .fetch_all(&self.pool)
        .await?;
        self.assemble(rows).await
    }

    async fn get(&self, id: i64) -> Result<Script> {
        let row = sqlx::query_as!(
            ScriptRow,
            r#"SELECT id as "id!", name, body, updated_at as "updated_at: DateTime<Utc>"
               FROM scripts WHERE id = ?"#,
            id
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("script {id}")))?;
        Ok(self.assemble(vec![row]).await?.remove(0))
    }

    async fn create(&self, script: &NewScript) -> Result<Script> {
        script.validate()?;
        let mut tx = self.pool.begin().await?;
        let id = write_script(&mut tx, None, script).await?;
        tx.commit().await?;
        self.get(id).await
    }

    async fn update(&self, id: i64, script: &NewScript) -> Result<Script> {
        script.validate()?;
        let mut tx = self.pool.begin().await?;
        write_script(&mut tx, Some(id), script).await?;
        tx.commit().await?;
        self.get(id).await
    }

    async fn delete(&self, id: i64) -> Result {
        let users = sqlx::query_scalar!(
            "SELECT DISTINCT i.name FROM image_scripts s JOIN images i ON i.id = s.image_id \
             WHERE s.script_id = ? ORDER BY i.name",
            id
        )
        .fetch_all(&self.pool)
        .await?;
        if !users.is_empty() {
            return Err(AppError::FailedPrecondition(format!(
                "script {id} is used by image {}",
                users.join(", ")
            )));
        }
        let deleted = sqlx::query!("DELETE FROM scripts WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        if deleted.rows_affected() == 0 {
            return Err(AppError::NotFound(format!("script {id}")));
        }
        Ok(())
    }

    async fn import_for_image(&self, image_id: i64, scripts: &[NewScript]) -> Result {
        for script in scripts {
            script.validate()?;
        }
        let mut tx = self.pool.begin().await?;
        let mut ids = Vec::with_capacity(scripts.len());
        for script in scripts {
            let existing = sqlx::query_scalar!(
                r#"SELECT id as "id!" FROM scripts WHERE name = ?"#,
                script.name
            )
            .fetch_optional(&mut *tx)
            .await?;
            ids.push(write_script(&mut tx, existing, script).await?);
        }
        link_scripts(&mut tx, image_id, &ids).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn get_for_image(&self, image_id: i64) -> Result<Vec<Script>> {
        let rows = sqlx::query_as!(
            ScriptRow,
            r#"SELECT s.id as "id!", s.name, s.body, s.updated_at as "updated_at: DateTime<Utc>"
               FROM image_scripts i JOIN scripts s ON s.id = i.script_id
               WHERE i.image_id = ?
               ORDER BY i.position"#,
            image_id
        )
        .fetch_all(&self.pool)
        .await?;
        self.assemble(rows).await
    }

    async fn set_for_image(&self, image_id: i64, script_ids: &[i64]) -> Result {
        let mut tx = self.pool.begin().await?;
        link_scripts(&mut tx, image_id, script_ids).await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn write_script(
    conn: &mut SqliteConnection,
    id: Option<i64>,
    script: &NewScript,
) -> Result<i64> {
    let now = Utc::now();
    let id = match id {
        None => {
            sqlx::query_scalar!(
                r#"INSERT INTO scripts (name, body, updated_at) VALUES (?, ?, ?) RETURNING id as "id!""#,
                script.name,
                script.body,
                now
            )
            .fetch_one(&mut *conn)
            .await?
        }
        Some(id) => {
            let updated = sqlx::query!(
                "UPDATE scripts SET name = ?, body = ?, updated_at = ? WHERE id = ?",
                script.name,
                script.body,
                now,
                id
            )
            .execute(&mut *conn)
            .await?;
            if updated.rows_affected() == 0 {
                return Err(AppError::NotFound(format!("script {id}")));
            }
            sqlx::query!("DELETE FROM script_inputs WHERE script_id = ?", id)
                .execute(&mut *conn)
                .await?;
            id
        }
    };
    insert_inputs(conn, id, script).await?;
    Ok(id)
}

async fn link_scripts(conn: &mut SqliteConnection, image_id: i64, script_ids: &[i64]) -> Result {
    sqlx::query_scalar!(
        "SELECT id FROM images WHERE id = ? AND deleted_at IS NULL",
        image_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("image {image_id}")))?;
    sqlx::query!("DELETE FROM image_scripts WHERE image_id = ?", image_id)
        .execute(&mut *conn)
        .await?;
    for (position, script_id) in script_ids.iter().enumerate() {
        if script_ids[..position].contains(script_id) {
            return Err(AppError::InvalidArgument(format!(
                "script {script_id} is listed twice"
            )));
        }
        sqlx::query_scalar!("SELECT id FROM scripts WHERE id = ?", script_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("script {script_id}")))?;
        let position = position as i64;
        sqlx::query!(
            "INSERT INTO image_scripts (image_id, position, script_id) VALUES (?, ?, ?)",
            image_id,
            position,
            script_id
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::di::DIContainer;
    use crate::domain::script::{InputKind, NewScript, ScriptInput};
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
            std::env::temp_dir().join(format!("imaged-reposcript-{}-{}", std::process::id(), id));
        let c = crate::build_test_container(&dir).await;
        (c, TestDir(dir))
    }

    fn input(name: &str, kind: InputKind, choices: &[&str]) -> ScriptInput {
        ScriptInput {
            name: name.to_string(),
            label: format!("The {name}"),
            kind,
            required: true,
            default_value: choices.first().map(|c| c.to_string()),
            per_machine: kind == InputKind::Text,
            choices: choices.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn new_script(name: &str, body: &str, inputs: Vec<ScriptInput>) -> NewScript {
        NewScript {
            name: name.to_string(),
            body: body.to_string(),
            inputs,
        }
    }

    #[tokio::test]
    async fn a_created_script_reads_back_with_its_inputs_in_declared_order() {
        let (c, _g) = container().await;
        let inputs = vec![
            input("zone", InputKind::Choice, &["eu", "us"]),
            input("hostname", InputKind::Text, &[]),
            input("count", InputKind::Number, &[]),
        ];
        let created = c
            .script_repo
            .create(&new_script("setup", "#!/bin/bash\necho", inputs.clone()))
            .await
            .unwrap();

        let fetched = c.script_repo.get(created.id).await.unwrap();
        assert_eq!(fetched.name, "setup");
        assert_eq!(fetched.body, "#!/bin/bash\necho");
        assert_eq!(fetched.inputs, inputs);
    }

    #[tokio::test]
    async fn scripts_are_listed_by_name_each_with_only_its_own_inputs() {
        let (c, _g) = container().await;
        c.script_repo
            .create(&new_script(
                "b",
                "x",
                vec![input("two", InputKind::Number, &[])],
            ))
            .await
            .unwrap();
        c.script_repo
            .create(&new_script(
                "a",
                "x",
                vec![input("one", InputKind::Number, &[])],
            ))
            .await
            .unwrap();

        let all = c.script_repo.get_all().await.unwrap();
        let listed: Vec<(&str, Vec<&str>)> = all
            .iter()
            .map(|s| {
                (
                    s.name.as_str(),
                    s.inputs.iter().map(|i| i.name.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(listed, vec![("a", vec!["one"]), ("b", vec!["two"])]);
    }

    #[tokio::test]
    async fn updating_replaces_name_body_and_inputs() {
        let (c, _g) = container().await;
        let created = c
            .script_repo
            .create(&new_script(
                "old",
                "x",
                vec![input("a", InputKind::Number, &[])],
            ))
            .await
            .unwrap();

        let updated = c
            .script_repo
            .update(
                created.id,
                &new_script("new", "y", vec![input("b", InputKind::Text, &[])]),
            )
            .await
            .unwrap();

        assert_eq!(updated.name, "new");
        assert_eq!(updated.body, "y");
        assert_eq!(updated.inputs, vec![input("b", InputKind::Text, &[])]);
        assert!(updated.updated_at >= created.updated_at);
    }

    #[tokio::test]
    async fn names_are_unique_and_invalid_definitions_are_not_stored() {
        let (c, _g) = container().await;
        c.script_repo
            .create(&new_script("setup", "x", vec![]))
            .await
            .unwrap();

        let duplicate = c
            .script_repo
            .create(&new_script("setup", "y", vec![]))
            .await
            .unwrap_err();
        assert!(
            matches!(duplicate, AppError::AlreadyExists(_)),
            "{duplicate:?}"
        );

        let invalid = c
            .script_repo
            .create(&new_script(
                "bad",
                "x",
                vec![input("Bad", InputKind::Text, &[])],
            ))
            .await
            .unwrap_err();
        assert!(
            matches!(invalid, AppError::InvalidArgument(_)),
            "{invalid:?}"
        );
        assert_eq!(c.script_repo.get_all().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unknown_scripts_are_not_found() {
        let (c, _g) = container().await;
        let script = new_script("s", "x", vec![]);
        assert!(matches!(
            c.script_repo.get(42).await.unwrap_err(),
            AppError::NotFound(_)
        ));
        assert!(matches!(
            c.script_repo.update(42, &script).await.unwrap_err(),
            AppError::NotFound(_)
        ));
        assert!(matches!(
            c.script_repo.delete(42).await.unwrap_err(),
            AppError::NotFound(_)
        ));
    }

    #[tokio::test]
    async fn an_image_runs_its_scripts_in_the_order_they_were_set() {
        let (c, _g) = container().await;
        let image = c.image_repo.create_image("img".into()).await.unwrap();
        let first = c
            .script_repo
            .create(&new_script("z", "1", vec![]))
            .await
            .unwrap();
        let second = c
            .script_repo
            .create(&new_script("a", "2", vec![]))
            .await
            .unwrap();

        c.script_repo
            .set_for_image(image.id, &[first.id, second.id])
            .await
            .unwrap();
        c.script_repo
            .set_for_image(image.id, &[second.id, first.id])
            .await
            .unwrap();

        let names: Vec<String> = c
            .script_repo
            .get_for_image(image.id)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["a", "z"]);
    }

    #[tokio::test]
    async fn a_rejected_script_list_leaves_the_previous_one_in_place() {
        let (c, _g) = container().await;
        let image = c.image_repo.create_image("img".into()).await.unwrap();
        let script = c
            .script_repo
            .create(&new_script("s", "1", vec![]))
            .await
            .unwrap();
        c.script_repo
            .set_for_image(image.id, &[script.id])
            .await
            .unwrap();

        let missing = c
            .script_repo
            .set_for_image(image.id, &[script.id, 999])
            .await
            .unwrap_err();
        assert!(matches!(missing, AppError::NotFound(_)), "{missing:?}");
        let twice = c
            .script_repo
            .set_for_image(image.id, &[script.id, script.id])
            .await
            .unwrap_err();
        assert!(matches!(twice, AppError::InvalidArgument(_)), "{twice:?}");
        let no_image = c
            .script_repo
            .set_for_image(999, &[script.id])
            .await
            .unwrap_err();
        assert!(matches!(no_image, AppError::NotFound(_)), "{no_image:?}");

        assert_eq!(
            c.script_repo.get_for_image(image.id).await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn a_script_used_by_an_image_cannot_be_deleted_until_the_image_is_gone() {
        let (c, _g) = container().await;
        let image = c.image_repo.create_image("golden".into()).await.unwrap();
        let script = c
            .script_repo
            .create(&new_script("s", "1", vec![]))
            .await
            .unwrap();
        c.script_repo
            .set_for_image(image.id, &[script.id])
            .await
            .unwrap();

        match c.script_repo.delete(script.id).await.unwrap_err() {
            AppError::FailedPrecondition(message) => assert!(message.contains("golden")),
            other => panic!("expected a failed precondition, got {other:?}"),
        }

        c.image_repo.delete_image(image.id).await.unwrap();
        c.script_repo.delete(script.id).await.unwrap();
        assert!(c.script_repo.get_all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn importing_replaces_scripts_by_name_and_attaches_them_in_order() {
        let (c, _g) = container().await;
        let other = c.image_repo.create_image("other".into()).await.unwrap();
        let imported = c.image_repo.create_image("imported".into()).await.unwrap();
        let original = c
            .script_repo
            .create(&new_script(
                "setup",
                "old",
                vec![input("a", InputKind::Number, &[])],
            ))
            .await
            .unwrap();
        c.script_repo
            .set_for_image(other.id, &[original.id])
            .await
            .unwrap();

        c.script_repo
            .import_for_image(
                imported.id,
                &[
                    new_script("added", "x", vec![]),
                    new_script("setup", "new", vec![]),
                ],
            )
            .await
            .unwrap();

        let attached = c.script_repo.get_for_image(imported.id).await.unwrap();
        let names: Vec<&str> = attached.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["added", "setup"]);
        assert_eq!(attached[1].id, original.id);
        let used = c.script_repo.get_for_image(other.id).await.unwrap();
        assert_eq!((used[0].body.as_str(), used[0].inputs.len()), ("new", 0));
    }

    #[tokio::test]
    async fn an_import_with_one_invalid_script_changes_nothing() {
        let (c, _g) = container().await;
        let image = c.image_repo.create_image("imported".into()).await.unwrap();
        c.script_repo
            .create(&new_script("setup", "old", vec![]))
            .await
            .unwrap();
        let twice = vec![
            input("x", InputKind::Number, &[]),
            input("x", InputKind::Number, &[]),
        ];

        let err = c
            .script_repo
            .import_for_image(
                image.id,
                &[
                    new_script("setup", "new", vec![]),
                    new_script("bad", "y", twice),
                ],
            )
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::InvalidArgument(_)), "{err:?}");
        let library = c.script_repo.get_all().await.unwrap();
        let listed: Vec<(&str, &str)> = library
            .iter()
            .map(|s| (s.name.as_str(), s.body.as_str()))
            .collect();
        assert_eq!(listed, [("setup", "old")]);
        assert!(
            c.script_repo
                .get_for_image(image.id)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
