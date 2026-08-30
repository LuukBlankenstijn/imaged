use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct BlockDevice {
    pub name: String,
    pub size: u64,
    #[serde(default)]
    pub fstype: Option<String>,
    #[serde(default)]
    pub mountpoint: Option<String>,
    #[serde(default, rename = "rm")]
    pub removable: bool,
    #[serde(default, rename = "tran")]
    pub transport: Option<String>,
    #[serde(default)]
    pub children: Vec<BlockDevice>,
}

#[derive(Debug, Clone)]
pub struct PartitionTarget {
    pub device: String,
    pub number: i64,
    pub fstype: String,
    pub size: u64,
}

impl PartitionTarget {
    pub fn partclone_binary(&self) -> Result<&'static str> {
        match self.fstype.as_str() {
            "ext2" | "ext3" | "ext4" => Ok("partclone.extfs"),
            "vfat" | "fat32" | "fat16" => Ok("partclone.vfat"),
            other => bail!("filetype not supported: {other}"),
        }
    }
}

impl BlockDevice {
    pub fn get_device(&self) -> String {
        format!("/dev/{}", self.name)
    }

    pub fn find_partition_number(&self) -> Result<i64> {
        let leading = self.name.trim_end_matches(|c: char| c.is_ascii_digit());
        self.name[leading.len()..]
            .parse()
            .with_context(|| format!("could not find partition number in {}", self.name))
    }

    pub fn formatted_partitions(&self) -> Result<Vec<PartitionTarget>> {
        let mut targets = Vec::new();
        for child in &self.children {
            let Some(fstype) = &child.fstype else {
                tracing::info!(name=%child.name, "skipping partition with no fstype");
                continue;
            };
            targets.push(PartitionTarget {
                device: child.get_device(),
                number: child.find_partition_number()?,
                fstype: fstype.clone(),
                size: child.size,
            });
        }
        Ok(targets)
    }

    pub fn partition_target(&self, number: i64, fstype: String) -> Result<PartitionTarget> {
        let child = self
            .children
            .iter()
            .find(|c| c.find_partition_number().is_ok_and(|n| n == number))
            .with_context(|| format!("partition {number} is missing from disk {}", self.name))?;
        Ok(PartitionTarget {
            device: child.get_device(),
            number,
            fstype,
            size: child.size,
        })
    }
}

#[derive(Debug, Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<BlockDevice>,
}

pub async fn find_target_disk() -> Result<BlockDevice> {
    let devices = lsblk(None).await?;

    let candidates: Vec<&BlockDevice> = devices
        .iter()
        .filter(|d| !d.removable && d.transport.as_deref() != Some("usb"))
        .collect();

    let disk = match candidates.as_slice() {
        [] => bail!("no suitable disk found"),
        [d] => *d,
        many => bail!(
            "multiple disks found: {:?}",
            many.iter().map(|d| d.name.clone()).collect::<Vec<_>>()
        ),
    };

    if contains_root(disk) {
        bail!(
            "refusing to image disk {}: contains the running root filesystem",
            disk.name
        );
    }

    Ok(disk.clone())
}

async fn lsblk(device: Option<&str>) -> Result<Vec<BlockDevice>> {
    let mut cmd = Command::new("lsblk");
    cmd.args(["-J", "-b", "-o", "NAME,SIZE,FSTYPE,MOUNTPOINT,RM,TRAN"]);
    if let Some(d) = device {
        cmd.arg(d);
    }

    let output = cmd.output().await.context("running lsblk")?;
    if !output.status.success() {
        bail!("lsblk failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(serde_json::from_slice::<LsblkOutput>(&output.stdout)
        .context("parsing lsblk output")?
        .blockdevices)
}

fn contains_root(dev: &BlockDevice) -> bool {
    dev.mountpoint.as_deref() == Some("/") || dev.children.iter().any(contains_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn freshly_partitioned_disk() -> BlockDevice {
        serde_json::from_str(
            r#"{
                "name": "nvme0n1",
                "size": 512110190592,
                "children": [
                    { "name": "nvme0n1p1", "size": 1073741824 },
                    { "name": "nvme0n1p2", "size": 511034359808 }
                ]
            }"#,
        )
        .expect("valid lsblk fixture")
    }

    #[test]
    fn restore_targets_do_not_depend_on_detected_fstype() {
        let disk = freshly_partitioned_disk();

        let target = disk
            .partition_target(2, "ext4".to_string())
            .expect("partition 2 exists");

        assert_eq!(target.device, "/dev/nvme0n1p2");
        assert_eq!(target.number, 2);
        assert_eq!(target.partclone_binary().unwrap(), "partclone.extfs");
    }

    #[test]
    fn restore_target_missing_from_disk_is_an_error() {
        let disk = freshly_partitioned_disk();

        assert!(disk.partition_target(3, "ext4".to_string()).is_err());
    }

    #[test]
    fn capture_skips_partitions_without_a_filesystem() {
        let disk = freshly_partitioned_disk();

        assert!(disk.formatted_partitions().unwrap().is_empty());
    }

    fn dev(json: &str) -> BlockDevice {
        serde_json::from_str(json).expect("valid block device fixture")
    }

    #[test]
    fn contains_root_detects_root_mounted_at_top_level() {
        let d = dev(r#"{ "name": "sda", "size": 1, "mountpoint": "/" }"#);
        assert!(contains_root(&d));
    }

    #[test]
    fn contains_root_detects_root_in_a_child_partition() {
        let d = dev(
            r#"{ "name": "sda", "size": 1, "children": [
                { "name": "sda1", "size": 1, "mountpoint": "/boot" },
                { "name": "sda2", "size": 1, "mountpoint": "/" }
            ] }"#,
        );
        assert!(contains_root(&d));
    }

    #[test]
    fn contains_root_detects_root_in_a_nested_child() {
        let d = dev(
            r#"{ "name": "sda", "size": 1, "children": [
                { "name": "sda1", "size": 1, "children": [
                    { "name": "vg-root", "size": 1, "mountpoint": "/" }
                ] }
            ] }"#,
        );
        assert!(contains_root(&d));
    }

    #[test]
    fn contains_root_is_false_when_no_partition_holds_root() {
        let d = dev(
            r#"{ "name": "sda", "size": 1, "children": [
                { "name": "sda1", "size": 1, "mountpoint": "/boot" },
                { "name": "sda2", "size": 1 }
            ] }"#,
        );
        assert!(!contains_root(&d));
    }

    #[tokio::test]
    async fn find_target_disk_never_returns_a_removable_usb_or_root_disk() {
        match find_target_disk().await {
            Ok(disk) => {
                assert!(!disk.removable, "selected a removable disk: {}", disk.name);
                assert_ne!(
                    disk.transport.as_deref(),
                    Some("usb"),
                    "selected a usb disk: {}",
                    disk.name
                );
                assert!(
                    !contains_root(&disk),
                    "selected the root disk: {}",
                    disk.name
                );
            }
            Err(e) => {
                let m = e.to_string();
                assert!(
                    m.contains("no suitable disk found")
                        || m.contains("multiple disks found")
                        || m.contains("refusing to image"),
                    "unexpected selection error: {m}"
                );
            }
        }
    }

    #[test]
    fn find_partition_number_reads_single_digit_partitions() {
        assert_eq!(dev(r#"{"name":"sda1","size":0}"#).find_partition_number().unwrap(), 1);
        assert_eq!(dev(r#"{"name":"nvme0n1p1","size":0}"#).find_partition_number().unwrap(), 1);
        assert_eq!(dev(r#"{"name":"mmcblk0p1","size":0}"#).find_partition_number().unwrap(), 1);
        assert_eq!(dev(r#"{"name":"loop0p1","size":0}"#).find_partition_number().unwrap(), 1);
    }

    #[test]
    fn find_partition_number_reads_multi_digit_partitions() {
        assert_eq!(dev(r#"{"name":"sda10","size":0}"#).find_partition_number().unwrap(), 10);
        assert_eq!(dev(r#"{"name":"nvme0n1p10","size":0}"#).find_partition_number().unwrap(), 10);
        assert_eq!(dev(r#"{"name":"mmcblk0p10","size":0}"#).find_partition_number().unwrap(), 10);
        assert_eq!(dev(r#"{"name":"sda12","size":0}"#).find_partition_number().unwrap(), 12);
        assert_eq!(dev(r#"{"name":"nvme0n1p11","size":0}"#).find_partition_number().unwrap(), 11);
        assert_eq!(dev(r#"{"name":"sda128","size":0}"#).find_partition_number().unwrap(), 128);
    }

    #[test]
    fn find_partition_number_ignores_digits_that_are_not_a_trailing_run() {
        assert_eq!(dev(r#"{"name":"nvme0n1p3","size":0}"#).find_partition_number().unwrap(), 3);
        assert_eq!(dev(r#"{"name":"mmcblk1p2","size":0}"#).find_partition_number().unwrap(), 2);
    }

    #[test]
    fn find_partition_number_errors_without_trailing_digits() {
        let e = dev(r#"{"name":"sda","size":0}"#).find_partition_number().unwrap_err();
        assert!(e.to_string().contains("could not find partition number in sda"));
        assert!(dev(r#"{"name":"","size":0}"#).find_partition_number().is_err());
    }

    #[test]
    fn find_partition_number_reads_an_all_digit_name_as_written() {
        assert_eq!(dev(r#"{"name":"123","size":0}"#).find_partition_number().unwrap(), 123);
    }

    #[test]
    fn partclone_binary_maps_supported_filesystems() {
        for fs in ["ext2", "ext3", "ext4"] {
            let t = PartitionTarget { device: "/dev/x".into(), number: 1, fstype: fs.into(), size: 0 };
            assert_eq!(t.partclone_binary().unwrap(), "partclone.extfs");
        }
        for fs in ["vfat", "fat32", "fat16"] {
            let t = PartitionTarget { device: "/dev/x".into(), number: 1, fstype: fs.into(), size: 0 };
            assert_eq!(t.partclone_binary().unwrap(), "partclone.vfat");
        }
    }

    #[test]
    fn partclone_binary_rejects_unmapped_filesystems() {
        for fs in ["ntfs", "xfs", "btrfs", "swap", ""] {
            let t = PartitionTarget { device: "/dev/x".into(), number: 1, fstype: fs.into(), size: 0 };
            let e = t.partclone_binary().unwrap_err();
            assert_eq!(e.to_string(), format!("filetype not supported: {fs}"));
        }
    }

    #[test]
    fn partclone_binary_is_case_sensitive() {
        for fs in ["EXT4", "Ext4", "VFAT"] {
            let t = PartitionTarget { device: "/dev/x".into(), number: 1, fstype: fs.into(), size: 0 };
            let e = t.partclone_binary().unwrap_err();
            assert_eq!(e.to_string(), format!("filetype not supported: {fs}"));
        }
    }

    #[test]
    fn formatted_partitions_follow_lsblk_order() {
        let disk = dev(
            r#"{ "name": "sdb", "size": 100, "children": [
                { "name": "sdb2", "size": 20, "fstype": "ext4" },
                { "name": "sdb1", "size": 10, "fstype": "vfat" }
            ] }"#,
        );
        let targets = disk.formatted_partitions().unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].device, "/dev/sdb2");
        assert_eq!(targets[0].number, 2);
        assert_eq!(targets[0].fstype, "ext4");
        assert_eq!(targets[1].device, "/dev/sdb1");
        assert_eq!(targets[1].number, 1);
    }

    #[test]
    fn formatted_partitions_error_on_unparseable_child_name() {
        let disk = dev(
            r#"{ "name": "sdb", "size": 100, "children": [
                { "name": "sdb", "size": 10, "fstype": "ext4" }
            ] }"#,
        );
        assert!(disk.formatted_partitions().is_err());
    }

    #[test]
    fn formatted_partitions_ignore_nested_children() {
        let disk = dev(
            r#"{ "name": "sda", "size": 100, "children": [
                { "name": "sda1", "size": 90, "fstype": "crypto_LUKS", "children": [
                    { "name": "crypted", "size": 89, "fstype": "ext4" }
                ] }
            ] }"#,
        );
        let targets = disk.formatted_partitions().unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].device, "/dev/sda1");
        assert_eq!(targets[0].fstype, "crypto_LUKS");
    }

    #[test]
    fn partition_target_carries_device_number_fstype_and_byte_size() {
        let disk = dev(
            r#"{ "name": "sda", "size": 100, "children": [
                { "name": "sda1", "size": 123456789, "fstype": "ext4" }
            ] }"#,
        );
        let target = disk.partition_target(1, "ext4".to_string()).unwrap();
        assert_eq!(target.device, "/dev/sda1");
        assert_eq!(target.number, 1);
        assert_eq!(target.fstype, "ext4");
        assert_eq!(target.size, 123456789);
    }

    #[test]
    fn partition_target_with_duplicate_numbers_silently_returns_the_first() {
        let disk = dev(
            r#"{ "name": "nvme0n1", "size": 100, "children": [
                { "name": "nvme0n1p1", "size": 111 },
                { "name": "nvme0n1p1", "size": 222 }
            ] }"#,
        );
        let target = disk.partition_target(1, "ext4".to_string()).unwrap();
        assert_eq!(target.device, "/dev/nvme0n1p1");
        assert_eq!(target.size, 111);
    }

    #[test]
    fn partition_target_distinguishes_single_from_multi_digit_siblings() {
        let disk = dev(
            r#"{ "name": "nvme0n1", "size": 100, "children": [
                { "name": "nvme0n1p1", "size": 111 },
                { "name": "nvme0n1p10", "size": 222 }
            ] }"#,
        );
        assert_eq!(disk.partition_target(1, "ext4".to_string()).unwrap().size, 111);
        assert_eq!(disk.partition_target(10, "ext4".to_string()).unwrap().size, 222);
    }
}
