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
            WHERE i.deleted_at IS NULL
            ORDER BY i.id, p.partition_number
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let mut images_map: HashMap<i64, Image> = HashMap::new();
        // Keep track of order since HashMap is unordered
        let mut image_ids = Vec::new();

        for row in rows {
            let entry = match images_map.get_mut(&row.image_id) {
                Some(entry) => entry,
                None => {
                    let id = row.image_id;
                    image_ids.push(id);
                    let image = Image::new(
                        id,
                        row.image_name,
                        row.captured_at,
                        ImageStatus::from_string(row.image_status)?,
                        row.error,
                        Vec::new(),
                    );
                    images_map.entry(id).or_insert(image)
                }
            };

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
                RETURNING
                    id as "id!: i64",
                    partition_number,
                    fstype,
                    size_bytes
            "#,
            image_id,
            partition_number,
            fstype,
            size_bytes,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(ImagePartition::new(
            partition.id,
            partition.partition_number,
            partition.fstype,
            partition.size_bytes as u64,
        ))
    }

    async fn delete_image(&self, id: i64) -> Result {
        // Soft delete: keep the row as a tombstone so tasks referencing this
        // image still resolve its name (and can distinguish "deleted" from
        // "no image"). The heavy partition blobs are reclaimed separately by
        // `image_service.clear_image_data`.
        let now = Utc::now();
        sqlx::query!("UPDATE images SET deleted_at = ? WHERE id = ?", now, id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn start_capture(&self, id: i64) -> Result {
        let status = ImageStatus::Capturing.to_string();
        let mut tx = self.pool.begin().await?;
        sqlx::query!("DELETE FROM image_partitions WHERE image_id = ?", id)
            .execute(&mut *tx)
            .await?;
        sqlx::query!("UPDATE images SET status = ? WHERE id = ?", status, id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

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
        Ok(sqlx::query!(
            r#"SELECT
                id as "id!: i64",
                partition_number,
                fstype,
                size_bytes
            FROM image_partitions WHERE image_id = ? ORDER BY partition_number"#,
            id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|partition| {
            ImagePartition::new(
                partition.id,
                partition.partition_number,
                partition.fstype,
                partition.size_bytes as u64,
            )
        })
        .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use sqlx::Row;

    use crate::di::DIContainer;
    use crate::domain::image::ImageStatus;
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
        let dir = std::env::temp_dir().join(format!(
            "imaged-repoimage-{}-{}",
            std::process::id(),
            id
        ));
        let c = crate::build_test_container(&dir).await;
        (c, TestDir(dir))
    }

    async fn raw_pool(guard: &TestDir) -> sqlx::SqlitePool {
        let url = format!("sqlite://{}", guard.0.join("test.db").display());
        sqlx::SqlitePool::connect(&url).await.unwrap()
    }

    async fn find_in_get_all(c: &DIContainer, id: i64) -> Option<crate::domain::image::Image> {
        c.image_repo
            .get_all()
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.id == id)
    }

    #[tokio::test]
    async fn create_image_starts_empty_with_name_and_no_capture_time() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("golden".into()).await.unwrap();
        assert_eq!(img.name, "golden");
        assert_eq!(img.status, ImageStatus::Empty);
        assert!(img.captured_at.is_none());
        assert!(img.error.is_none());
        assert!(img.partitions.is_empty());
    }

    #[tokio::test]
    async fn get_status_of_freshly_created_image_is_empty() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("golden".into()).await.unwrap();
        let status = c.image_repo.get_status(img.id).await.unwrap();
        assert_eq!(status, ImageStatus::Empty);
    }

    #[tokio::test]
    async fn get_status_of_unknown_id_maps_to_not_found() {
        let (c, _g) = container().await;
        let err = c.image_repo.get_status(999_999).await.unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn get_all_orders_images_by_id_ascending() {
        let (c, _g) = container().await;
        let a = c.image_repo.create_image("a".into()).await.unwrap();
        let b = c.image_repo.create_image("b".into()).await.unwrap();
        let cc = c.image_repo.create_image("c".into()).await.unwrap();
        let ids: Vec<i64> = c.image_repo.get_all().await.unwrap().iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![a.id, b.id, cc.id]);
    }

    #[tokio::test]
    async fn get_all_groups_partitions_onto_their_image_ordered_by_partition_number() {
        let (c, _g) = container().await;
        let img1 = c.image_repo.create_image("one".into()).await.unwrap();
        let img2 = c.image_repo.create_image("two".into()).await.unwrap();
        c.image_repo.save_partition(img1.id, 3, "ext4", 30).await.unwrap();
        c.image_repo.save_partition(img1.id, 1, "vfat", 10).await.unwrap();
        c.image_repo.save_partition(img1.id, 2, "ext4", 20).await.unwrap();
        c.image_repo.save_partition(img2.id, 1, "ext4", 99).await.unwrap();

        let all = c.image_repo.get_all().await.unwrap();
        let one = all.iter().find(|i| i.id == img1.id).unwrap();
        let two = all.iter().find(|i| i.id == img2.id).unwrap();

        let nums: Vec<i64> = one.partitions.iter().map(|p| p.partition_number).collect();
        assert_eq!(nums, vec![1, 2, 3]);
        assert_eq!(two.partitions.len(), 1);
        assert_eq!(two.partitions[0].partition_number, 1);
    }

    #[tokio::test]
    async fn get_all_excludes_soft_deleted_images() {
        let (c, _g) = container().await;
        let keep = c.image_repo.create_image("keep".into()).await.unwrap();
        let gone = c.image_repo.create_image("gone".into()).await.unwrap();
        c.image_repo.delete_image(gone.id).await.unwrap();

        let ids: Vec<i64> = c.image_repo.get_all().await.unwrap().iter().map(|i| i.id).collect();
        assert!(ids.contains(&keep.id));
        assert!(!ids.contains(&gone.id));
    }

    #[tokio::test]
    async fn save_partition_round_trips_fstype_and_size_through_get_partitions() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.save_partition(img.id, 1, "ext4", 4096).await.unwrap();

        let parts = c.image_repo.get_partitions(img.id).await.unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].partition_number, 1);
        assert_eq!(parts[0].fstype, "ext4");
        assert_eq!(parts[0].size_bytes, 4096);
    }

    #[tokio::test]
    async fn get_partitions_orders_by_partition_number() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.save_partition(img.id, 5, "ext4", 1).await.unwrap();
        c.image_repo.save_partition(img.id, 2, "ext4", 1).await.unwrap();
        c.image_repo.save_partition(img.id, 8, "ext4", 1).await.unwrap();

        let nums: Vec<i64> = c
            .image_repo
            .get_partitions(img.id)
            .await
            .unwrap()
            .iter()
            .map(|p| p.partition_number)
            .collect();
        assert_eq!(nums, vec![2, 5, 8]);
    }

    #[tokio::test]
    async fn save_partition_return_value_reports_the_partition_number_it_was_given() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        let returned = c.image_repo.save_partition(img.id, 7, "ext4", 1).await.unwrap();
        assert_eq!(returned.partition_number, 7);
        let stored = c.image_repo.get_partitions(img.id).await.unwrap();
        assert_eq!(stored[0].partition_number, 7);
    }

    #[tokio::test]
    async fn re_saving_the_same_partition_number_errors_on_the_unique_constraint() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.save_partition(img.id, 1, "ext4", 10).await.unwrap();
        let again = c.image_repo.save_partition(img.id, 1, "vfat", 20).await;
        assert!(again.is_err(), "expected UNIQUE(image_id, partition_number) violation");

        let parts = c.image_repo.get_partitions(img.id).await.unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].fstype, "ext4");
        assert_eq!(parts[0].size_bytes, 10);
    }

    #[tokio::test]
    async fn size_bytes_near_i64_max_round_trips_as_u64() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.save_partition(img.id, 1, "ext4", i64::MAX).await.unwrap();
        let parts = c.image_repo.get_partitions(img.id).await.unwrap();
        assert_eq!(parts[0].size_bytes, i64::MAX as u64);
    }

    #[tokio::test]
    async fn a_negative_size_bytes_is_reachable_and_wraps_to_a_huge_u64() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.save_partition(img.id, 1, "ext4", -1).await.unwrap();
        let parts = c.image_repo.get_partitions(img.id).await.unwrap();
        assert_eq!(parts[0].size_bytes, u64::MAX);
    }

    #[tokio::test]
    async fn a_freshly_inserted_partition_always_has_a_non_null_id() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        let returned = c.image_repo.save_partition(img.id, 1, "ext4", 1).await.unwrap();
        assert!(returned.id > 0);
        let parts = c.image_repo.get_partitions(img.id).await.unwrap();
        assert!(parts.iter().all(|p| p.id > 0));
    }

    #[tokio::test]
    async fn get_all_returns_an_internal_error_when_an_image_row_has_an_unexpected_status_string() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        let pool = raw_pool(&_g).await;
        sqlx::query("UPDATE images SET status = ? WHERE id = ?")
            .bind("bogus")
            .bind(img.id)
            .execute(&pool)
            .await
            .unwrap();
        let err = c.image_repo.get_all().await.unwrap_err();
        assert!(matches!(err, AppError::Internal(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn get_status_of_an_unexpected_status_string_is_an_internal_error_not_a_panic() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        let pool = raw_pool(&_g).await;
        sqlx::query("UPDATE images SET status = ? WHERE id = ?")
            .bind("bogus")
            .bind(img.id)
            .execute(&pool)
            .await
            .unwrap();
        let err = c.image_repo.get_status(img.id).await.unwrap_err();
        assert!(matches!(err, AppError::Internal(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn start_capture_moves_an_empty_image_to_capturing() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.start_capture(img.id).await.unwrap();
        assert_eq!(c.image_repo.get_status(img.id).await.unwrap(), ImageStatus::Capturing);
    }

    #[tokio::test]
    async fn mark_finished_moves_capturing_to_ready_and_records_a_capture_time() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.start_capture(img.id).await.unwrap();
        c.image_repo.mark_finished(img.id).await.unwrap();
        assert_eq!(c.image_repo.get_status(img.id).await.unwrap(), ImageStatus::Ready);
        let reloaded = find_in_get_all(&c, img.id).await.unwrap();
        assert!(reloaded.captured_at.is_some());
    }

    #[tokio::test]
    async fn mark_faulted_moves_capturing_to_faulted_and_records_the_error() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.start_capture(img.id).await.unwrap();
        c.image_repo.mark_faulted(img.id, "disk exploded").await.unwrap();
        assert_eq!(c.image_repo.get_status(img.id).await.unwrap(), ImageStatus::Faulted);
        let reloaded = find_in_get_all(&c, img.id).await.unwrap();
        assert_eq!(reloaded.error.as_deref(), Some("disk exploded"));
    }

    #[tokio::test]
    async fn transitions_are_unconditional_so_mark_finished_promotes_an_empty_image_to_ready() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.mark_finished(img.id).await.unwrap();
        assert_eq!(c.image_repo.get_status(img.id).await.unwrap(), ImageStatus::Ready);
    }

    #[tokio::test]
    async fn mark_faulted_overwrites_a_ready_image_from_its_unexpected_source_state() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.mark_finished(img.id).await.unwrap();
        c.image_repo.mark_faulted(img.id, "regressed").await.unwrap();
        assert_eq!(c.image_repo.get_status(img.id).await.unwrap(), ImageStatus::Faulted);
    }

    #[tokio::test]
    async fn start_capture_atomically_clears_partitions_and_flips_status_to_capturing() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.save_partition(img.id, 1, "ext4", 10).await.unwrap();
        c.image_repo.save_partition(img.id, 2, "vfat", 20).await.unwrap();
        c.image_repo.mark_finished(img.id).await.unwrap();

        c.image_repo.start_capture(img.id).await.unwrap();

        assert_eq!(c.image_repo.get_status(img.id).await.unwrap(), ImageStatus::Capturing);
        assert!(c.image_repo.get_partitions(img.id).await.unwrap().is_empty());
        let reloaded = find_in_get_all(&c, img.id).await.unwrap();
        assert_eq!(reloaded.status, ImageStatus::Capturing);
        assert!(reloaded.partitions.is_empty());
    }

    #[tokio::test]
    async fn delete_image_is_a_soft_delete_that_keeps_the_row_name_and_sets_deleted_at() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("survivor".into()).await.unwrap();
        c.image_repo.delete_image(img.id).await.unwrap();

        let pool = raw_pool(&_g).await;
        let row = sqlx::query("SELECT name, deleted_at FROM images WHERE id = ?")
            .bind(img.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let name: String = row.get("name");
        let deleted_at: Option<String> = row.get("deleted_at");
        assert_eq!(name, "survivor");
        assert!(deleted_at.is_some());
    }

    #[tokio::test]
    async fn get_status_still_resolves_a_soft_deleted_image_because_it_has_no_deleted_at_filter() {
        let (c, _g) = container().await;
        let img = c.image_repo.create_image("img".into()).await.unwrap();
        c.image_repo.mark_finished(img.id).await.unwrap();
        c.image_repo.delete_image(img.id).await.unwrap();
        assert_eq!(c.image_repo.get_status(img.id).await.unwrap(), ImageStatus::Ready);
    }

}
