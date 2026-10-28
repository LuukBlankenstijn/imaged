use std::collections::HashMap;

use chrono::{DateTime, Utc};
use derive_more::Constructor;
use imaged_model::script::{InputKind, MAX_SCRIPT_NAME_LEN, ScriptInput};
use sqlx::{Executor, Sqlite, SqliteConnection, SqlitePool, types::Json};

use crate::domain::script::{ImageScript, NewImageScript, NewScript, Script, ScriptRepository};
use imaged_model::error::{AppError, Result};

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
    description: Option<String>,
    kind: String,
    required: bool,
    default_value: Option<String>,
    per_machine: bool,
    choices: Json<Vec<String>>,
}

impl SqliteScriptRepository {
    async fn assemble(&self, rows: Vec<ScriptRow>) -> Result<Vec<Script>> {
        let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
        let mut inputs = load_inputs(&self.pool, &ids).await?;
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

async fn load_inputs<'e, E>(executor: E, ids: &[i64]) -> Result<HashMap<i64, Vec<ScriptInput>>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let ids = Json(ids);
    let mut inputs: HashMap<i64, Vec<ScriptInput>> = HashMap::new();
    for row in sqlx::query_as!(
        InputRow,
        r#"SELECT
             script_id,
             name,
             label,
             description,
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
    .fetch_all(executor)
    .await?
    {
        inputs.entry(row.script_id).or_default().push(ScriptInput {
            name: row.name,
            label: row.label,
            description: row.description,
            kind: input_kind(&row.kind)?,
            required: row.required,
            default_value: row.default_value,
            per_machine: row.per_machine,
            choices: row.choices.0,
        });
    }
    Ok(inputs)
}

fn input_kind(value: &str) -> Result<InputKind> {
    value.parse::<InputKind>().map_err(|e| {
        tracing::info!(err=%e, "failed to convert string to InputKind");
        AppError::Internal("conversion error".to_string())
    })
}

async fn insert_inputs(conn: &mut SqliteConnection, script_id: i64, script: &NewScript) -> Result {
    for (position, input) in script.inputs.iter().enumerate() {
        let position = position as i64;
        let kind = input.kind.to_string();
        let choices = Json(&input.choices);
        sqlx::query!(
            "INSERT INTO script_inputs \
             (script_id, position, name, label, description, kind, required, default_value, per_machine, choices) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            script_id,
            position,
            input.name,
            input.label,
            input.description,
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
        let id = write_script(&mut tx, None, &script.name, script).await?;
        tx.commit().await?;
        self.get(id).await
    }

    async fn update(&self, id: i64, script: &NewScript) -> Result<Script> {
        script.validate()?;
        let mut tx = self.pool.begin().await?;
        write_script(&mut tx, Some(id), &script.name, script).await?;
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

    async fn import_for_image(&self, image_id: i64, scripts: &[NewImageScript]) -> Result {
        for imported in scripts {
            imported.script.validate()?;
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut linked = Vec::with_capacity(scripts.len());
        for imported in scripts {
            let id = import_script(&mut tx, image_id, &imported.script).await?;
            linked.push((id, imported.root_partition));
        }
        link_scripts(&mut tx, image_id, &linked).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn get_for_image(&self, image_id: i64) -> Result<Vec<ImageScript>> {
        let (rows, roots): (Vec<ScriptRow>, Vec<Option<i64>>) = sqlx::query!(
            r#"SELECT s.id as "id!", s.name, s.body, s.updated_at as "updated_at: DateTime<Utc>",
                      i.root_partition
               FROM image_scripts i JOIN scripts s ON s.id = i.script_id
               WHERE i.image_id = ?
               ORDER BY i.position"#,
            image_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            (
                ScriptRow {
                    id: row.id,
                    name: row.name,
                    body: row.body,
                    updated_at: row.updated_at,
                },
                row.root_partition,
            )
        })
        .unzip();
        Ok(self
            .assemble(rows)
            .await?
            .into_iter()
            .zip(roots)
            .map(|(script, root_partition)| ImageScript {
                script,
                root_partition,
            })
            .collect())
    }

    async fn set_for_image(&self, image_id: i64, script_ids: &[i64]) -> Result {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let roots: HashMap<i64, Option<i64>> = sqlx::query!(
            "SELECT script_id, root_partition FROM image_scripts WHERE image_id = ?",
            image_id
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| (row.script_id, row.root_partition))
        .collect();
        let linked: Vec<(i64, Option<i64>)> = script_ids
            .iter()
            .map(|id| (*id, roots.get(id).copied().flatten()))
            .collect();
        link_scripts(&mut tx, image_id, &linked).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn set_root_for_image(
        &self,
        image_id: i64,
        position: i64,
        partition_number: Option<i64>,
    ) -> Result {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(number) = partition_number {
            let archived = sqlx::query_scalar!(
                r#"SELECT EXISTS (
                     SELECT 1 FROM image_partitions WHERE image_id = ? AND partition_number = ?
                   ) as "exists!: bool""#,
                image_id,
                number
            )
            .fetch_one(&mut *tx)
            .await?;
            if !archived {
                return Err(AppError::InvalidArgument(format!(
                    "image {image_id} has no partition {number}"
                )));
            }
        }
        let updated = sqlx::query!(
            "UPDATE image_scripts SET root_partition = ? WHERE image_id = ? AND position = ?",
            partition_number,
            image_id,
            position
        )
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(AppError::NotFound(format!(
                "image {image_id} has no script at position {position}"
            )));
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn import_script(
    conn: &mut SqliteConnection,
    image_id: i64,
    script: &NewScript,
) -> Result<i64> {
    let mut attempt = 1;
    loop {
        let name = import_name(&script.name, attempt);
        let Some(existing) = sqlx::query!(
            r#"SELECT id as "id!", body FROM scripts WHERE name = ?"#,
            name
        )
        .fetch_optional(&mut *conn)
        .await?
        else {
            return write_script(conn, None, &name, script).await;
        };
        let inputs = load_inputs(&mut *conn, &[existing.id])
            .await?
            .remove(&existing.id)
            .unwrap_or_default();
        if existing.body == script.body && inputs == script.inputs {
            return Ok(existing.id);
        }
        if attempt == 1 && !used_by_other_images(conn, existing.id, image_id).await? {
            return write_script(conn, Some(existing.id), &name, script).await;
        }
        attempt += 1;
    }
}

async fn used_by_other_images(
    conn: &mut SqliteConnection,
    script_id: i64,
    image_id: i64,
) -> Result<bool> {
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (
             SELECT 1 FROM image_scripts WHERE script_id = ? AND image_id <> ?
           ) as "used!: bool""#,
        script_id,
        image_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

fn import_name(name: &str, attempt: u32) -> String {
    if attempt == 1 {
        return name.to_string();
    }
    let suffix = format!("-{attempt}");
    let base = &name[..name.len().min(MAX_SCRIPT_NAME_LEN - suffix.len())];
    format!("{base}{suffix}")
}

async fn write_script(
    conn: &mut SqliteConnection,
    id: Option<i64>,
    name: &str,
    script: &NewScript,
) -> Result<i64> {
    let now = Utc::now();
    let id = match id {
        None => {
            sqlx::query_scalar!(
                r#"INSERT INTO scripts (name, body, updated_at) VALUES (?, ?, ?) RETURNING id as "id!""#,
                name,
                script.body,
                now
            )
            .fetch_one(&mut *conn)
            .await?
        }
        Some(id) => {
            let updated = sqlx::query!(
                "UPDATE scripts SET name = ?, body = ?, updated_at = ? WHERE id = ?",
                name,
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

async fn link_scripts(
    conn: &mut SqliteConnection,
    image_id: i64,
    scripts: &[(i64, Option<i64>)],
) -> Result {
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
    for (position, (script_id, root_partition)) in scripts.iter().enumerate() {
        if scripts[..position].iter().any(|(id, _)| id == script_id) {
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
            "INSERT INTO image_scripts (image_id, position, script_id, root_partition) VALUES (?, ?, ?, ?)",
            image_id,
            position,
            script_id,
            root_partition
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::domain::script::{NewImageScript, NewScript};
    use crate::test_support::repos;
    use imaged_model::error::AppError;
    use imaged_model::image::ImageKind;
    use imaged_model::script::{InputKind, MAX_SCRIPT_NAME_LEN, ScriptInput};

    fn input(name: &str, kind: InputKind, choices: &[&str]) -> ScriptInput {
        ScriptInput {
            name: name.to_string(),
            label: format!("The {name}"),
            description: (kind == InputKind::Text).then(|| format!("What {name} to use")),
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

    fn attachable<const N: usize>(scripts: [NewScript; N]) -> Vec<NewImageScript> {
        scripts
            .into_iter()
            .map(|script| NewImageScript {
                script,
                root_partition: None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_created_script_reads_back_with_its_inputs_in_declared_order() {
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
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
        let (c, _g) = repos().await;
        let image = c
            .image_repo
            .create_image("img".into(), ImageKind::Disk)
            .await
            .unwrap();
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
            .map(|s| s.script.name)
            .collect();
        assert_eq!(names, vec!["a", "z"]);
    }

    #[tokio::test]
    async fn a_rejected_script_list_leaves_the_previous_one_in_place() {
        let (c, _g) = repos().await;
        let image = c
            .image_repo
            .create_image("img".into(), ImageKind::Disk)
            .await
            .unwrap();
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
    async fn a_script_root_override_names_an_image_partition_and_stays_with_its_script() {
        let (c, _g) = repos().await;
        let image = c
            .image_repo
            .create_image("img".into(), ImageKind::Disk)
            .await
            .unwrap();
        c.image_repo
            .save_partition(image.id, 2, "ext4", 1, None, None)
            .await
            .unwrap();
        let first = c
            .script_repo
            .create(&new_script("a", "1", vec![]))
            .await
            .unwrap();
        let second = c
            .script_repo
            .create(&new_script("b", "2", vec![]))
            .await
            .unwrap();
        let roots = || async {
            c.script_repo
                .get_for_image(image.id)
                .await
                .unwrap()
                .into_iter()
                .map(|s| (s.script.name, s.root_partition))
                .collect::<Vec<_>>()
        };
        c.script_repo
            .set_for_image(image.id, &[first.id, second.id])
            .await
            .unwrap();

        c.script_repo
            .set_root_for_image(image.id, 1, Some(2))
            .await
            .unwrap();
        let unknown_partition = c
            .script_repo
            .set_root_for_image(image.id, 0, Some(3))
            .await
            .unwrap_err();
        assert!(
            matches!(unknown_partition, AppError::InvalidArgument(_)),
            "{unknown_partition:?}"
        );
        let unknown_position = c
            .script_repo
            .set_root_for_image(image.id, 2, None)
            .await
            .unwrap_err();
        assert!(
            matches!(unknown_position, AppError::NotFound(_)),
            "{unknown_position:?}"
        );
        assert_eq!(roots().await, [("a".into(), None), ("b".into(), Some(2))]);

        c.script_repo
            .set_for_image(image.id, &[second.id, first.id])
            .await
            .unwrap();
        assert_eq!(roots().await, [("b".into(), Some(2)), ("a".into(), None)]);

        c.script_repo
            .set_for_image(image.id, &[first.id])
            .await
            .unwrap();
        c.script_repo
            .set_for_image(image.id, &[first.id, second.id])
            .await
            .unwrap();
        assert_eq!(roots().await, [("a".into(), None), ("b".into(), None)]);

        c.script_repo
            .set_root_for_image(image.id, 0, Some(2))
            .await
            .unwrap();
        c.image_repo.start_capture(image.id, 512).await.unwrap();
        assert_eq!(roots().await, [("a".into(), None), ("b".into(), None)]);
    }

    #[tokio::test]
    async fn a_script_used_by_an_image_cannot_be_deleted_until_the_image_is_gone() {
        let (c, _g) = repos().await;
        let image = c
            .image_repo
            .create_image("golden".into(), ImageKind::Disk)
            .await
            .unwrap();
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
    async fn importing_a_changed_script_another_image_uses_attaches_a_renamed_copy() {
        let (c, _g) = repos().await;
        let other = c
            .image_repo
            .create_image("other".into(), ImageKind::Disk)
            .await
            .unwrap();
        let imported = c
            .image_repo
            .create_image("imported".into(), ImageKind::Disk)
            .await
            .unwrap();
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
                &attachable([
                    new_script("added", "x", vec![]),
                    new_script("setup", "new", vec![]),
                ]),
            )
            .await
            .unwrap();

        let attached = c.script_repo.get_for_image(imported.id).await.unwrap();
        let listed: Vec<(&str, &str)> = attached
            .iter()
            .map(|s| (s.script.name.as_str(), s.script.body.as_str()))
            .collect();
        assert_eq!(listed, [("added", "x"), ("setup-2", "new")]);
        let used = &c.script_repo.get_for_image(other.id).await.unwrap()[0].script;
        assert_eq!(used.id, original.id);
        assert_eq!((used.name.as_str(), used.body.as_str()), ("setup", "old"));
        assert_eq!(used.inputs, original.inputs);
    }

    #[tokio::test]
    async fn importing_the_same_colliding_script_again_reuses_its_renamed_copy() {
        let (c, _g) = repos().await;
        let other = c
            .image_repo
            .create_image("other".into(), ImageKind::Disk)
            .await
            .unwrap();
        let original = c
            .script_repo
            .create(&new_script("setup", "old", vec![]))
            .await
            .unwrap();
        c.script_repo
            .set_for_image(other.id, &[original.id])
            .await
            .unwrap();
        let archived = attachable([new_script("setup", "new", vec![])]);

        let mut attached = Vec::new();
        for name in ["first", "second"] {
            let image = c
                .image_repo
                .create_image(name.into(), ImageKind::Disk)
                .await
                .unwrap();
            c.script_repo
                .import_for_image(image.id, &archived)
                .await
                .unwrap();
            attached.push(
                c.script_repo.get_for_image(image.id).await.unwrap()[0]
                    .script
                    .id,
            );
        }

        assert_eq!(attached[0], attached[1]);
        let names: Vec<String> = c
            .script_repo
            .get_all()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["setup", "setup-2"]);
    }

    #[tokio::test]
    async fn a_renamed_import_skips_an_unrelated_library_script_that_holds_the_suffixed_name() {
        let (c, _g) = repos().await;
        let other = c
            .image_repo
            .create_image("other".into(), ImageKind::Disk)
            .await
            .unwrap();
        let imported = c
            .image_repo
            .create_image("imported".into(), ImageKind::Disk)
            .await
            .unwrap();
        let original = c
            .script_repo
            .create(&new_script("setup", "old", vec![]))
            .await
            .unwrap();
        c.script_repo
            .set_for_image(other.id, &[original.id])
            .await
            .unwrap();
        let unrelated = c
            .script_repo
            .create(&new_script("setup-2", "mine", vec![]))
            .await
            .unwrap();

        c.script_repo
            .import_for_image(
                imported.id,
                &attachable([new_script("setup", "new", vec![])]),
            )
            .await
            .unwrap();

        let attached = c.script_repo.get_for_image(imported.id).await.unwrap();
        assert_eq!(
            (
                attached[0].script.name.as_str(),
                attached[0].script.body.as_str()
            ),
            ("setup-3", "new")
        );
        assert_eq!(c.script_repo.get(unrelated.id).await.unwrap().body, "mine");
    }

    #[tokio::test]
    async fn importing_reuses_an_identical_script_and_replaces_one_no_other_image_uses() {
        let (c, _g) = repos().await;
        let other = c
            .image_repo
            .create_image("other".into(), ImageKind::Disk)
            .await
            .unwrap();
        let imported = c
            .image_repo
            .create_image("imported".into(), ImageKind::Disk)
            .await
            .unwrap();
        let inputs = vec![input("mode", InputKind::Choice, &["fast", "safe"])];
        let shared = c
            .script_repo
            .create(&new_script("setup", "same", inputs.clone()))
            .await
            .unwrap();
        c.script_repo
            .set_for_image(other.id, &[shared.id])
            .await
            .unwrap();
        let unused = c
            .script_repo
            .create(&new_script("cleanup", "old", vec![]))
            .await
            .unwrap();

        c.script_repo
            .import_for_image(
                imported.id,
                &attachable([
                    new_script("setup", "same", inputs),
                    new_script("cleanup", "new", vec![]),
                ]),
            )
            .await
            .unwrap();

        let attached = c.script_repo.get_for_image(imported.id).await.unwrap();
        let listed: Vec<(i64, &str, &str)> = attached
            .iter()
            .map(|s| (s.script.id, s.script.name.as_str(), s.script.body.as_str()))
            .collect();
        assert_eq!(
            listed,
            [(shared.id, "setup", "same"), (unused.id, "cleanup", "new")]
        );
        assert_eq!(c.script_repo.get_all().await.unwrap().len(), 2);
    }

    #[test]
    fn a_renamed_import_stays_within_the_name_limit() {
        let long = "a".repeat(MAX_SCRIPT_NAME_LEN);
        let renamed = super::import_name(&long, 12);
        assert_eq!(renamed.len(), MAX_SCRIPT_NAME_LEN);
        assert!(renamed.ends_with("a-12"), "{renamed}");
        assert_eq!(super::import_name("setup", 1), "setup");
    }

    #[tokio::test]
    async fn an_import_with_one_invalid_script_changes_nothing() {
        let (c, _g) = repos().await;
        let image = c
            .image_repo
            .create_image("imported".into(), ImageKind::Disk)
            .await
            .unwrap();
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
                &attachable([
                    new_script("setup", "new", vec![]),
                    new_script("bad", "y", twice),
                ]),
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
