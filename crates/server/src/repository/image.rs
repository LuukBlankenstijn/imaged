use std::collections::HashMap;

use crate::{domain::image::ImagePartition, error::Result};
use chrono::{DateTime, Utc};
use derive_more::Constructor;
use sqlx::SqlitePool;

use crate::domain::image::{Image, ImageRepository, ImageStatus};

#[derive(Debug, Constructor)]
pub struct SqliteImageRepository {
    pool: SqlitePool,
}

#[async_trait::async_trait]
impl ImageRepository for SqliteImageRepository {
    async fn get_status(&self, id: i64) -> Result<ImageStatus> {
        let result = sqlx::query!("SELECT status FROM images WHERE id = ?", id)
            .fetch_one(&self.pool)
            .await?;
        return ImageStatus::from_string(result.status);
    }
    async fn create_image(&self, name: String) -> Result<Image> {
        let status = ImageStatus::Empty.to_string();
        let image = sqlx::query!(
            "INSERT INTO images (name, status) VALUES (?,?) RETURNING *",
            name,
            status
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(Image::new(
            image.id,
            image.name,
            None,
            ImageStatus::from_string(image.status)?,
            image.error,
            Vec::new(),
        ))
    }
    async fn update_name(&self, id: i64, name: String) -> Result<Image> {
        let image = sqlx::query!(
            r#"UPDATE images SET name = ? WHERE id = ?
               RETURNING
                 id as "id!: i64",
                 name,
                 captured_at as "captured_at: DateTime<Utc>",
                 status,
                 error
            "#,
            name,
            id
        )
        .fetch_one(&self.pool)
        .await?;

        let partitions = sqlx::query!(
            r#"SELECT
                id as "id!: i64",
                partition_number,
                fstype,
                size_bytes
            FROM image_partitions WHERE image_id = ?"#,
            id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|record| {
            ImagePartition::new(
                record.id,
                record.partition_number,
                record.fstype,
                record.size_bytes as u64,
            )
        })
        .collect();

        Ok(Image::new(
            image.id,
            image.name,
            image.captured_at,
            ImageStatus::from_string(image.status)?,
            image.error,
            partitions,
        ))
    }

    async fn get_all(&self) -> Result<Vec<Image>> {
        // 1. Fetch everything in one JOIN query
        // We use a LEFT JOIN so images with zero partitions are still included
        let rows = sqlx::query!(
            r#"
            SELECT 
                i.id AS "image_id!",
                i.name AS "image_name!",
                i.status AS "image_status!",
                i.error AS "error",
                i.captured_at as "captured_at: DateTime<Utc>",
                p.id AS "p_id?: i64",
                p.partition_number AS "p_num?: i64",
                p.fstype AS "p_fstype?",
                p.size_bytes AS "p_size?: i64"
            FROM images i
            LEFT JOIN image_partitions p ON i.id = p.image_id
            ORDER BY i.id, p.partition_number
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let mut images_map: HashMap<i64, Image> = HashMap::new();
        // Keep track of order since HashMap is unordered
        let mut image_ids = Vec::new();

        for row in rows {
            let entry = images_map.entry(row.image_id).or_insert_with(|| {
                image_ids.push(row.image_id);
                Image::new(
                    row.image_id,
                    row.image_name,
                    row.captured_at,
                    ImageStatus::from_string(row.image_status)
                        .expect("image status error, should never happen"),
                    row.error,
                    Vec::new(),
                )
            });

            // 3. Add partition if it exists (the LEFT JOIN will yield nulls for p_* if empty)
            if let (Some(p_id), Some(p_num), Some(p_fstype), Some(p_size)) =
                (row.p_id, row.p_num, row.p_fstype, row.p_size)
            {
                entry
                    .partitions
                    .push(ImagePartition::new(p_id, p_num, p_fstype, p_size as u64));
            }
        }

        // 4. Transform back into a Vec ordered by the original query
        let result = image_ids
            .into_iter()
            .filter_map(|id| images_map.remove(&id))
            .collect();

        Ok(result)
    }

    async fn save_partition(
        &self,
        image_id: i64,
        partition_number: i64,
        fstype: &str,
        size_bytes: i64,
    ) -> Result<ImagePartition> {
        let partition = sqlx::query!(
            r#"
                INSERT INTO image_partitions 
                    (image_id, partition_number, fstype, size_bytes) 
                VALUES 
                    (?,?,?,?)
                RETURNING *
            "#,
            image_id,
            partition_number,
            fstype,
            size_bytes,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(ImagePartition::new(
            // for some reason id is an option
            partition.id.unwrap(),
            partition.image_id,
            partition.fstype,
            partition.size_bytes as u64,
        ))
    }

    async fn delete_image(&self, id: i64) -> Result {
        sqlx::query!("DELETE FROM images WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn start_capture(&self, id: i64) -> Result {
        let status = ImageStatus::Capturing.to_string();
        sqlx::query!("DELETE FROM image_partitions WHERE image_id = ?", id)
            .execute(&self.pool)
            .await?;
        sqlx::query!("UPDATE images SET status = ? WHERE id = ?", status, id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn mark_finished(&self, id: i64) -> Result {
        let status = ImageStatus::Ready.to_string();
        let now = Utc::now();
        sqlx::query!(
            "UPDATE images SET status = ?, captured_at = ? WHERE id = ?",
            status,
            now,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_faulted(&self, id: i64, error: &str) -> Result {
        let status = ImageStatus::Faulted.to_string();
        sqlx::query!(
            "UPDATE images SET status = ?, error = ? WHERE id = ?",
            status,
            error,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_partitions(&self, id: i64) -> Result<Vec<ImagePartition>> {
        Ok(
            sqlx::query!("SELECT * FROM image_partitions WHERE image_id = ? ORDER BY partition_number", id)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(|partition| {
                    ImagePartition::new(
                        // for some reason id is an option
                        partition.id.unwrap(),
                        partition.partition_number,
                        partition.fstype,
                        partition.size_bytes as u64,
                    )
                })
                .collect(),
        )
    }
}
