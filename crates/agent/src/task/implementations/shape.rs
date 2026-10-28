use std::path::Path;

use anyhow::{Context as _, Result, bail, ensure};
use derive_more::Constructor;
use imaged_disk::{Filesystem, backup_partition_table, restore_partition_table};
use imaged_model::agent::ImageLayout;

use super::PARTTABLE_TMP;
use super::source::Source;
use crate::agent::Agent;
use crate::sys::disk::{BlockDevice, PartitionTarget, find_target_disk};
use crate::sys::fstab::Matching;

const IMAGE_PARTITION: i64 = 1;

#[allow(
    async_fn_in_trait,
    reason = "only awaited through the concrete shapes held by the Task enum, whose futures are Send"
)]
pub trait Shape: Clone {
    const NAME: &'static str;
    const FSTAB: Matching;

    async fn capture_table(&self, agent: &Agent, task_id: i64, disk: &BlockDevice) -> Result<()>;

    fn capture_plan(&self, disk: &BlockDevice) -> Result<Vec<PartitionTarget>>;

    async fn restore_table(
        &self,
        source: &impl Source,
        agent: &Agent,
        task_id: i64,
        disk: BlockDevice,
        layout: &ImageLayout,
    ) -> Result<BlockDevice>;

    fn restore_plan(
        &self,
        disk: &BlockDevice,
        layout: &ImageLayout,
    ) -> Result<Vec<PartitionTarget>>;
}

#[derive(Clone, Copy, Debug)]
pub struct WholeDisk;

impl Shape for WholeDisk {
    const NAME: &'static str = "disk";
    const FSTAB: Matching = Matching::First;

    async fn capture_table(&self, agent: &Agent, task_id: i64, disk: &BlockDevice) -> Result<()> {
        backup_partition_table(Path::new(&disk.get_device()), Path::new(PARTTABLE_TMP)).await?;
        let table = tokio::fs::read(PARTTABLE_TMP).await?;
        agent
            .server
            .upload_partition_table(task_id, table.into())
            .await?;
        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        Ok(())
    }

    fn capture_plan(&self, disk: &BlockDevice) -> Result<Vec<PartitionTarget>> {
        disk.formatted_partitions()
    }

    async fn restore_table(
        &self,
        source: &impl Source,
        agent: &Agent,
        task_id: i64,
        disk: BlockDevice,
        layout: &ImageLayout,
    ) -> Result<BlockDevice> {
        let table = source.partition_table(agent, task_id).await?;
        tokio::fs::write(PARTTABLE_TMP, table).await?;
        restore_partition_table(
            Path::new(&disk.get_device()),
            Path::new(PARTTABLE_TMP),
            layout.sector_size,
        )
        .await?;
        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        find_target_disk().await
    }

    fn restore_plan(
        &self,
        disk: &BlockDevice,
        layout: &ImageLayout,
    ) -> Result<Vec<PartitionTarget>> {
        layout
            .partitions
            .iter()
            .map(|image| disk.partition_target(image))
            .collect()
    }
}

#[derive(Clone, Debug, Constructor)]
pub struct SinglePartition {
    partuuid: String,
}

impl Shape for SinglePartition {
    const NAME: &'static str = "partition";
    const FSTAB: Matching = Matching::Unique;

    async fn capture_table(&self, _: &Agent, _: i64, _: &BlockDevice) -> Result<()> {
        Ok(())
    }

    fn capture_plan(&self, disk: &BlockDevice) -> Result<Vec<PartitionTarget>> {
        let source = disk.partition(&self.partuuid)?;
        let filesystem = Filesystem::detect(source.fstype.as_deref())?
            .with_context(|| format!("partition {} holds no filesystem", self.partuuid))?;
        if !(filesystem.is_ext() || filesystem == Filesystem::Vfat) {
            bail!(
                "partition {} holds {filesystem}, a partition image holds an ext2, ext3, ext4 or vfat filesystem",
                self.partuuid
            );
        }
        Ok(vec![PartitionTarget {
            device: source.get_device(),
            number: IMAGE_PARTITION,
            filesystem,
            size: source.size,
            fs_uuid: source.uuid.clone(),
            fs_label: source.label.clone(),
        }])
    }

    async fn restore_table(
        &self,
        _: &impl Source,
        _: &Agent,
        _: i64,
        disk: BlockDevice,
        _: &ImageLayout,
    ) -> Result<BlockDevice> {
        Ok(disk)
    }

    fn restore_plan(
        &self,
        disk: &BlockDevice,
        layout: &ImageLayout,
    ) -> Result<Vec<PartitionTarget>> {
        let [image] = layout.partitions.as_slice() else {
            bail!(
                "a partition image holds exactly one partition, this one holds {}",
                layout.partitions.len()
            );
        };
        ensure!(
            image.partition_number == IMAGE_PARTITION,
            "a partition image holds partition {IMAGE_PARTITION}, this one holds partition {}",
            image.partition_number
        );
        let target = disk.partition(&self.partuuid)?;
        ensure!(
            target.size >= image.size_bytes,
            "partition {} holds {} bytes, the image needs at least {}",
            self.partuuid,
            target.size,
            image.size_bytes
        );
        let sector_size = disk.logical_sector_size()?;
        ensure!(
            sector_size == layout.sector_size,
            "the image is for {}-byte sectors, {} has {sector_size}-byte sectors",
            layout.sector_size,
            disk.name
        );
        let twin = |holds: &dyn Fn(&BlockDevice) -> bool| {
            disk.children
                .iter()
                .find(|child| !std::ptr::eq(*child, target) && holds(child))
        };
        if let Some(fs_uuid) = &image.fs_uuid
            && let Some(other) = twin(&|child| {
                child
                    .uuid
                    .as_deref()
                    .is_some_and(|uuid| uuid.eq_ignore_ascii_case(fs_uuid))
            })
        {
            bail!(
                "{} already holds a filesystem with UUID {fs_uuid}, restoring the image into partition {} would duplicate it",
                other.get_device(),
                self.partuuid
            );
        }
        if let Some(fs_label) = &image.fs_label
            && let Some(other) = twin(&|child| child.label.as_ref() == Some(fs_label))
        {
            bail!(
                "{} already holds a filesystem labelled `{fs_label}`, restoring the image into partition {} would duplicate the label",
                other.get_device(),
                self.partuuid
            );
        }
        Ok(vec![PartitionTarget {
            device: target.get_device(),
            number: IMAGE_PARTITION,
            filesystem: image.fstype.parse()?,
            size: target.size,
            fs_uuid: image.fs_uuid.clone(),
            fs_label: image.fs_label.clone(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use imaged_model::agent::ImagePartition;

    use super::*;

    fn disk() -> BlockDevice {
        serde_json::from_str(
            r#"{ "name": "nvme0n1", "size": 1000000, "log-sec": 512, "children": [
                { "name": "nvme0n1p1", "size": 1000, "fstype": "vfat", "uuid": "ABCD-1234",
                  "partuuid": "aaaa-01" },
                { "name": "nvme0n1p2", "size": 5000, "fstype": "ext4", "uuid": "old-root",
                  "label": "root fs", "partuuid": "aaaa-02" },
                { "name": "nvme0n1p3", "size": 3000, "fstype": "swap", "uuid": "swap",
                  "partuuid": "aaaa-03" },
                { "name": "nvme0n1p4", "size": 4000, "partuuid": "aaaa-04" }
            ] }"#,
        )
        .expect("valid lsblk fixture")
    }

    fn layout(size_bytes: u64, fs_uuid: &str) -> ImageLayout {
        ImageLayout {
            sector_size: 512,
            partitions: vec![ImagePartition {
                id: 9,
                partition_number: 1,
                fstype: "ext4".to_string(),
                size_bytes,
                fs_uuid: Some(fs_uuid.to_string()),
                fs_label: Some("fresh".to_string()),
            }],
        }
    }

    fn labelled(mut layout: ImageLayout, fs_label: &str) -> ImageLayout {
        layout.partitions[0].fs_label = Some(fs_label.to_string());
        layout
    }

    fn into(partuuid: &str) -> SinglePartition {
        SinglePartition::new(partuuid.to_string())
    }

    fn refusal(partuuid: &str, disk: &BlockDevice, layout: &ImageLayout) -> String {
        into(partuuid)
            .restore_plan(disk, layout)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn an_image_restores_as_partition_one_onto_the_host_partition_it_fits() {
        let [target] = into("AAAA-04")
            .restore_plan(&disk(), &layout(4000, "new-root"))
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(target.device, "/dev/nvme0n1p4");
        assert_eq!(target.number, 1);
        assert_eq!(target.filesystem, Filesystem::Ext4);
        assert_eq!(target.size, 4000);
        assert_eq!(target.fs_uuid.as_deref(), Some("new-root"));
        assert_eq!(target.fs_label.as_deref(), Some("fresh"));
    }

    #[test]
    fn redeploying_over_the_partition_that_already_holds_the_image_is_allowed() {
        let planned =
            into("aaaa-02").restore_plan(&disk(), &labelled(layout(4000, "OLD-ROOT"), "root fs"));

        assert_eq!(planned.unwrap()[0].device, "/dev/nvme0n1p2");
    }

    #[test]
    fn a_target_that_is_missing_too_small_or_differently_sectored_is_refused() {
        let disk = disk();
        assert_eq!(
            refusal("bbbb-01", &disk, &layout(1, "x")),
            "partition bbbb-01 is not on disk nvme0n1"
        );
        assert_eq!(
            refusal("aaaa-04", &disk, &layout(4001, "x")),
            "partition aaaa-04 holds 4000 bytes, the image needs at least 4001"
        );
        let mut other_sectors = layout(1, "x");
        other_sectors.sector_size = 4096;
        assert_eq!(
            refusal("aaaa-04", &disk, &other_sectors),
            "the image is for 4096-byte sectors, nvme0n1 has 512-byte sectors"
        );
    }

    #[test]
    fn a_target_beside_a_partition_holding_the_images_filesystem_uuid_is_refused() {
        assert_eq!(
            refusal("aaaa-04", &disk(), &layout(1, "old-root")),
            "/dev/nvme0n1p2 already holds a filesystem with UUID old-root, restoring the image into partition aaaa-04 would duplicate it"
        );
    }

    #[test]
    fn a_target_beside_a_partition_holding_the_images_filesystem_label_is_refused() {
        assert_eq!(
            refusal(
                "aaaa-04",
                &disk(),
                &labelled(layout(1, "new-root"), "root fs")
            ),
            "/dev/nvme0n1p2 already holds a filesystem labelled `root fs`, restoring the image into partition aaaa-04 would duplicate the label"
        );
        let planned =
            into("aaaa-04").restore_plan(&disk(), &labelled(layout(1, "new-root"), "ROOT FS"));
        assert_eq!(planned.unwrap()[0].fs_label.as_deref(), Some("ROOT FS"));
    }

    #[test]
    fn only_an_image_of_exactly_partition_one_is_restored() {
        let mut two = layout(1, "x");
        two.partitions.push(two.partitions[0].clone());
        assert_eq!(
            refusal("aaaa-04", &disk(), &two),
            "a partition image holds exactly one partition, this one holds 2"
        );
        let mut second = layout(1, "x");
        second.partitions[0].partition_number = 2;
        assert_eq!(
            refusal("aaaa-04", &disk(), &second),
            "a partition image holds partition 1, this one holds partition 2"
        );
    }

    #[test]
    fn a_captured_partition_becomes_partition_one_with_its_size_and_filesystem_identity() {
        let [source] = into("aaaa-02")
            .capture_plan(&disk())
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(source.device, "/dev/nvme0n1p2");
        assert_eq!(source.number, 1);
        assert_eq!(source.filesystem, Filesystem::Ext4);
        assert_eq!(source.size, 5000);
        assert_eq!(source.fs_uuid.as_deref(), Some("old-root"));
        assert_eq!(source.fs_label.as_deref(), Some("root fs"));
    }

    #[test]
    fn only_ext_and_vfat_partitions_are_captured() {
        let capture = |partuuid: &str| into(partuuid).capture_plan(&disk()).map(|p| p.len());

        assert_eq!(capture("aaaa-01").unwrap(), 1);
        assert_eq!(
            capture("aaaa-03").unwrap_err().to_string(),
            "partition aaaa-03 holds swap, a partition image holds an ext2, ext3, ext4 or vfat filesystem"
        );
        assert_eq!(
            capture("aaaa-04").unwrap_err().to_string(),
            "partition aaaa-04 holds no filesystem"
        );
    }
}
