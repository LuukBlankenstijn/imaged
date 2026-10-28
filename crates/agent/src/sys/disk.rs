use anyhow::{Context as _, Result, bail};
use imaged_disk::Filesystem;
use imaged_model::agent::ImagePartition;
use imaged_model::host::HostPartition;
use serde::Deserialize;
use tokio::process::Command;

const E2FSCK_CONFIG: &str = "/etc/e2fsck.conf";
const E2FSCK_REPAIRED_REBOOT: i32 = 2;
const FSCK_FAT_REPAIRED: i32 = 1;
const BTRFS_CHECK_CLEAN: i32 = 0;

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
    #[serde(default)]
    pub ro: bool,
    #[serde(default)]
    pub hotplug: bool,
    #[serde(default, rename = "type")]
    pub device_type: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub partuuid: Option<String>,
    #[serde(default)]
    pub partlabel: Option<String>,
    #[serde(default, rename = "log-sec")]
    pub sector_size: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct PartitionTarget {
    pub device: String,
    pub number: i64,
    pub filesystem: Filesystem,
    pub size: u64,
    pub fs_uuid: Option<String>,
    pub fs_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostDisk {
    pub size_bytes: u64,
    pub partitions: Vec<HostPartition>,
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
            let Some(filesystem) = Filesystem::detect(child.fstype.as_deref())? else {
                tracing::info!(name=%child.name, "skipping partition with no fstype");
                continue;
            };
            targets.push(PartitionTarget {
                device: child.get_device(),
                number: child.find_partition_number()?,
                filesystem,
                size: child.size,
                fs_uuid: child.uuid.clone(),
                fs_label: child.label.clone(),
            });
        }
        Ok(targets)
    }

    pub fn partition_target(&self, image: &ImagePartition) -> Result<PartitionTarget> {
        let number = image.partition_number;
        let child = self
            .children
            .iter()
            .find(|c| c.find_partition_number().is_ok_and(|n| n == number))
            .with_context(|| format!("partition {number} is missing from disk {}", self.name))?;
        Ok(PartitionTarget {
            device: child.get_device(),
            number,
            filesystem: image.fstype.parse()?,
            size: child.size,
            fs_uuid: image.fs_uuid.clone(),
            fs_label: image.fs_label.clone(),
        })
    }

    pub fn partition(&self, partuuid: &str) -> Result<&BlockDevice> {
        self.children
            .iter()
            .find(|child| {
                child
                    .partuuid
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(partuuid))
            })
            .with_context(|| format!("partition {partuuid} is not on disk {}", self.name))
    }

    pub fn logical_sector_size(&self) -> Result<u32> {
        self.sector_size
            .with_context(|| format!("lsblk reported no logical sector size for {}", self.name))
    }

    pub fn host_partitions(&self) -> Vec<HostPartition> {
        self.children
            .iter()
            .filter_map(|child| {
                Some(HostPartition {
                    number: child.find_partition_number().ok()?,
                    partuuid: child.partuuid.clone()?,
                    partlabel: child.partlabel.clone(),
                    label: child.label.clone(),
                    uuid: child.uuid.clone(),
                    fstype: child.fstype.clone(),
                    size_bytes: child.size,
                })
            })
            .collect()
    }
}

pub async fn probe() -> std::result::Result<HostDisk, String> {
    let disk = find_target_disk().await.map_err(|e| format!("{e:#}"))?;
    Ok(HostDisk {
        size_bytes: disk.size,
        partitions: disk.host_partitions(),
    })
}

#[derive(Debug, Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<BlockDevice>,
}

fn is_imaging_candidate(d: &BlockDevice) -> bool {
    d.device_type.as_deref() == Some("disk")
        && !d.ro
        && !d.hotplug
        && !d.removable
        && d.transport.as_deref() != Some("usb")
}

pub async fn find_target_disk() -> Result<BlockDevice> {
    let devices = lsblk(None).await?;

    let candidates: Vec<&BlockDevice> =
        devices.iter().filter(|d| is_imaging_candidate(d)).collect();

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
    cmd.args([
        "-J",
        "-b",
        "-o",
        "NAME,SIZE,FSTYPE,MOUNTPOINT,RM,TRAN,RO,HOTPLUG,TYPE,UUID,LABEL,PARTUUID,PARTLABEL,LOG-SEC",
    ]);
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

pub async fn repair_filesystem(partition: &PartitionTarget) -> Result<()> {
    let (mut command, worst_accepted) = match partition.filesystem {
        Filesystem::Swap => return Ok(()),
        Filesystem::Vfat => {
            let mut command = Command::new("fsck.fat");
            command.arg("-a");
            (command, FSCK_FAT_REPAIRED)
        }
        Filesystem::Ext2 | Filesystem::Ext3 | Filesystem::Ext4 => {
            let mut command = Command::new("e2fsck");
            command.env("E2FSCK_CONFIG", E2FSCK_CONFIG).arg("-p");
            (command, E2FSCK_REPAIRED_REBOOT)
        }
        Filesystem::Btrfs => {
            let mut command = Command::new("btrfs");
            command.args(["check", "--readonly"]);
            (command, BTRFS_CHECK_CLEAN)
        }
    };
    tracing::info!(device = %partition.device, filesystem = %partition.filesystem, "checking the filesystem before capturing it");
    let output = command
        .arg(&partition.device)
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("checking {}", partition.device))?;
    if output
        .status
        .code()
        .is_some_and(|code| code <= worst_accepted)
    {
        return Ok(());
    }
    let printed = [output.stdout, output.stderr].concat();
    bail!(
        "{} has filesystem errors that could not be repaired automatically ({}): {}",
        partition.device,
        output.status,
        String::from_utf8_lossy(&printed).trim()
    )
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

    fn image(number: i64, fstype: &str) -> ImagePartition {
        ImagePartition {
            id: number,
            partition_number: number,
            fstype: fstype.to_string(),
            size_bytes: 1,
            fs_uuid: Some(format!("uuid-{number}")),
            fs_label: Some(format!("label-{number}")),
        }
    }

    #[test]
    fn restore_targets_do_not_depend_on_detected_fstype() {
        let disk = freshly_partitioned_disk();

        let target = disk
            .partition_target(&image(2, "ext4"))
            .expect("partition 2 exists");

        assert_eq!(target.device, "/dev/nvme0n1p2");
        assert_eq!(target.number, 2);
        assert_eq!(target.filesystem, Filesystem::Ext4);
    }

    #[test]
    fn restore_target_missing_from_disk_is_an_error() {
        let disk = freshly_partitioned_disk();

        assert!(disk.partition_target(&image(3, "ext4")).is_err());
    }

    #[test]
    fn capture_skips_partitions_without_a_filesystem() {
        let disk = freshly_partitioned_disk();

        assert!(disk.formatted_partitions().unwrap().is_empty());
    }

    #[test]
    fn capture_targets_carry_the_filesystem_identity_and_restore_targets_the_images() {
        let disk = dev(r#"{ "name": "sda", "size": 100, "children": [
                { "name": "sda1", "size": 10, "fstype": "vfat", "uuid": "ABCD-1234", "label": "ESP" },
                { "name": "sda2", "size": 90, "fstype": "ext4" }
            ] }"#);

        let captured: Vec<_> = disk
            .formatted_partitions()
            .unwrap()
            .into_iter()
            .map(|target| (target.fs_uuid, target.fs_label))
            .collect();
        assert_eq!(
            captured,
            [
                (Some("ABCD-1234".to_string()), Some("ESP".to_string())),
                (None, None)
            ]
        );
        let restored = disk.partition_target(&image(2, "ext4")).unwrap();
        assert_eq!(
            (restored.fs_uuid, restored.fs_label),
            (Some("uuid-2".to_string()), Some("label-2".to_string()))
        );
    }

    #[test]
    fn the_inventory_lists_numbered_partitions_that_have_a_partuuid() {
        let disk = dev(
            r#"{ "name": "nvme0n1", "size": 100, "log-sec": 4096, "children": [
                { "name": "nvme0n1p1", "size": 10, "fstype": "vfat", "uuid": "ABCD-1234",
                  "label": "ESP", "partuuid": "aaaa-01", "partlabel": "EFI system" },
                { "name": "nvme0n1p2", "size": 80 },
                { "name": "nvme0n1p3", "size": 9, "partuuid": "aaaa-03" },
                { "name": "crypted", "size": 1, "partuuid": "aaaa-00" }
            ] }"#,
        );

        assert_eq!(
            disk.host_partitions(),
            [
                HostPartition {
                    number: 1,
                    partuuid: "aaaa-01".to_string(),
                    partlabel: Some("EFI system".to_string()),
                    label: Some("ESP".to_string()),
                    uuid: Some("ABCD-1234".to_string()),
                    fstype: Some("vfat".to_string()),
                    size_bytes: 10,
                },
                HostPartition {
                    number: 3,
                    partuuid: "aaaa-03".to_string(),
                    partlabel: None,
                    label: None,
                    uuid: None,
                    fstype: None,
                    size_bytes: 9,
                },
            ]
        );
        assert_eq!(disk.logical_sector_size().unwrap(), 4096);
        assert_eq!(disk.partition("AAAA-03").unwrap().name, "nvme0n1p3");
        assert_eq!(
            disk.partition("bbbb-01").unwrap_err().to_string(),
            "partition bbbb-01 is not on disk nvme0n1"
        );
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
        let d = dev(r#"{ "name": "sda", "size": 1, "children": [
                { "name": "sda1", "size": 1, "mountpoint": "/boot" },
                { "name": "sda2", "size": 1, "mountpoint": "/" }
            ] }"#);
        assert!(contains_root(&d));
    }

    #[test]
    fn contains_root_detects_root_in_a_nested_child() {
        let d = dev(r#"{ "name": "sda", "size": 1, "children": [
                { "name": "sda1", "size": 1, "children": [
                    { "name": "vg-root", "size": 1, "mountpoint": "/" }
                ] }
            ] }"#);
        assert!(contains_root(&d));
    }

    #[test]
    fn contains_root_is_false_when_no_partition_holds_root() {
        let d = dev(r#"{ "name": "sda", "size": 1, "children": [
                { "name": "sda1", "size": 1, "mountpoint": "/boot" },
                { "name": "sda2", "size": 1 }
            ] }"#);
        assert!(!contains_root(&d));
    }

    fn internal_disk() -> BlockDevice {
        dev(
            r#"{ "name": "nvme0n1", "size": 1, "type": "disk", "rm": false, "ro": false,
                 "hotplug": false, "tran": "nvme" }"#,
        )
    }

    #[test]
    fn an_internal_writable_disk_is_an_imaging_candidate() {
        assert!(is_imaging_candidate(&internal_disk()));
    }

    #[test]
    fn emmc_boot_partitions_are_not_imaging_candidates() {
        let boot0 = dev(
            r#"{ "name": "mmcblk0boot0", "size": 4194304, "type": "disk",
                 "rm": false, "ro": true, "hotplug": false, "tran": null }"#,
        );
        assert!(!is_imaging_candidate(&boot0));
    }

    #[test]
    fn hotplug_sd_cards_are_not_imaging_candidates() {
        let sd_card = dev(r#"{ "name": "mmcblk1", "size": 1, "type": "disk",
                 "rm": false, "ro": false, "hotplug": true, "tran": null }"#);
        assert!(!is_imaging_candidate(&sd_card));
    }

    #[test]
    fn non_disk_devices_are_not_imaging_candidates() {
        for device_type in [Some("rom"), Some("part"), Some("loop"), Some("lvm"), None] {
            let device = BlockDevice {
                device_type: device_type.map(str::to_string),
                ..internal_disk()
            };
            assert!(!is_imaging_candidate(&device), "{device_type:?}");
        }
    }

    #[test]
    fn removable_and_usb_disks_are_not_imaging_candidates() {
        let removable = BlockDevice {
            removable: true,
            ..internal_disk()
        };
        let usb = BlockDevice {
            transport: Some("usb".to_string()),
            ..internal_disk()
        };
        assert!(!is_imaging_candidate(&removable));
        assert!(!is_imaging_candidate(&usb));
    }

    #[tokio::test]
    async fn find_target_disk_only_returns_an_imaging_candidate_without_root() {
        match find_target_disk().await {
            Ok(disk) => {
                assert!(
                    is_imaging_candidate(&disk),
                    "selected an excluded disk: {disk:?}"
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
        assert_eq!(
            dev(r#"{"name":"sda1","size":0}"#)
                .find_partition_number()
                .unwrap(),
            1
        );
        assert_eq!(
            dev(r#"{"name":"nvme0n1p1","size":0}"#)
                .find_partition_number()
                .unwrap(),
            1
        );
        assert_eq!(
            dev(r#"{"name":"mmcblk0p1","size":0}"#)
                .find_partition_number()
                .unwrap(),
            1
        );
        assert_eq!(
            dev(r#"{"name":"loop0p1","size":0}"#)
                .find_partition_number()
                .unwrap(),
            1
        );
    }

    #[test]
    fn find_partition_number_reads_multi_digit_partitions() {
        assert_eq!(
            dev(r#"{"name":"sda10","size":0}"#)
                .find_partition_number()
                .unwrap(),
            10
        );
        assert_eq!(
            dev(r#"{"name":"nvme0n1p10","size":0}"#)
                .find_partition_number()
                .unwrap(),
            10
        );
        assert_eq!(
            dev(r#"{"name":"mmcblk0p10","size":0}"#)
                .find_partition_number()
                .unwrap(),
            10
        );
        assert_eq!(
            dev(r#"{"name":"sda12","size":0}"#)
                .find_partition_number()
                .unwrap(),
            12
        );
        assert_eq!(
            dev(r#"{"name":"nvme0n1p11","size":0}"#)
                .find_partition_number()
                .unwrap(),
            11
        );
        assert_eq!(
            dev(r#"{"name":"sda128","size":0}"#)
                .find_partition_number()
                .unwrap(),
            128
        );
    }

    #[test]
    fn find_partition_number_ignores_digits_that_are_not_a_trailing_run() {
        assert_eq!(
            dev(r#"{"name":"nvme0n1p3","size":0}"#)
                .find_partition_number()
                .unwrap(),
            3
        );
        assert_eq!(
            dev(r#"{"name":"mmcblk1p2","size":0}"#)
                .find_partition_number()
                .unwrap(),
            2
        );
    }

    #[test]
    fn find_partition_number_errors_without_trailing_digits() {
        let e = dev(r#"{"name":"sda","size":0}"#)
            .find_partition_number()
            .unwrap_err();
        assert!(
            e.to_string()
                .contains("could not find partition number in sda")
        );
        assert!(
            dev(r#"{"name":"","size":0}"#)
                .find_partition_number()
                .is_err()
        );
    }

    #[test]
    fn find_partition_number_reads_an_all_digit_name_as_written() {
        assert_eq!(
            dev(r#"{"name":"123","size":0}"#)
                .find_partition_number()
                .unwrap(),
            123
        );
    }

    #[test]
    fn formatted_partitions_follow_lsblk_order() {
        let disk = dev(r#"{ "name": "sdb", "size": 100, "children": [
                { "name": "sdb2", "size": 20, "fstype": "ext4" },
                { "name": "sdb1", "size": 10, "fstype": "vfat" }
            ] }"#);
        let targets = disk.formatted_partitions().unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].device, "/dev/sdb2");
        assert_eq!(targets[0].number, 2);
        assert_eq!(targets[0].filesystem, Filesystem::Ext4);
        assert_eq!(targets[1].device, "/dev/sdb1");
        assert_eq!(targets[1].number, 1);
        assert_eq!(targets[1].filesystem, Filesystem::Vfat);
    }

    #[test]
    fn formatted_partitions_error_on_unparseable_child_name() {
        let disk = dev(r#"{ "name": "sdb", "size": 100, "children": [
                { "name": "sdb", "size": 10, "fstype": "ext4" }
            ] }"#);
        assert!(disk.formatted_partitions().is_err());
    }

    #[test]
    fn formatted_partitions_ignore_nested_children() {
        let disk = dev(r#"{ "name": "sda", "size": 100, "children": [
                { "name": "sda1", "size": 90, "children": [
                    { "name": "crypted", "size": 89, "fstype": "ext4" }
                ] },
                { "name": "sda2", "size": 10, "fstype": "vfat" }
            ] }"#);
        let targets = disk.formatted_partitions().unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].device, "/dev/sda2");
    }

    #[test]
    fn formatted_partitions_reject_unsupported_filesystems() {
        let disk = dev(r#"{ "name": "sda", "size": 100, "children": [
                { "name": "sda1", "size": 10, "fstype": "vfat" },
                { "name": "sda2", "size": 90, "fstype": "crypto_LUKS" }
            ] }"#);
        let e = disk.formatted_partitions().unwrap_err();
        assert_eq!(e.to_string(), "filesystem not supported: crypto_LUKS");
    }

    #[test]
    fn partition_target_rejects_an_unsupported_filesystem() {
        let disk = freshly_partitioned_disk();

        let e = disk.partition_target(&image(2, "ntfs")).unwrap_err();
        assert_eq!(e.to_string(), "filesystem not supported: ntfs");
    }

    #[test]
    fn partition_target_carries_device_number_fstype_and_byte_size() {
        let disk = dev(r#"{ "name": "sda", "size": 100, "children": [
                { "name": "sda1", "size": 123456789, "fstype": "ext4" }
            ] }"#);
        let target = disk.partition_target(&image(1, "ext4")).unwrap();
        assert_eq!(target.device, "/dev/sda1");
        assert_eq!(target.number, 1);
        assert_eq!(target.filesystem, Filesystem::Ext4);
        assert_eq!(target.size, 123456789);
    }

    #[test]
    fn partition_target_with_duplicate_numbers_silently_returns_the_first() {
        let disk = dev(r#"{ "name": "nvme0n1", "size": 100, "children": [
                { "name": "nvme0n1p1", "size": 111 },
                { "name": "nvme0n1p1", "size": 222 }
            ] }"#);
        let target = disk.partition_target(&image(1, "ext4")).unwrap();
        assert_eq!(target.device, "/dev/nvme0n1p1");
        assert_eq!(target.size, 111);
    }

    #[test]
    fn partition_target_distinguishes_single_from_multi_digit_siblings() {
        let disk = dev(r#"{ "name": "nvme0n1", "size": 100, "children": [
                { "name": "nvme0n1p1", "size": 111 },
                { "name": "nvme0n1p10", "size": 222 }
            ] }"#);
        assert_eq!(disk.partition_target(&image(1, "ext4")).unwrap().size, 111);
        assert_eq!(disk.partition_target(&image(10, "ext4")).unwrap().size, 222);
    }

    fn btrfs_target(dir: &std::path::Path, name: &str) -> PartitionTarget {
        let image = dir.join(name);
        std::fs::File::create(&image)
            .unwrap()
            .set_len(128 << 20)
            .unwrap();
        let formatted = std::process::Command::new("mkfs.btrfs")
            .arg("-q")
            .arg(&image)
            .status()
            .unwrap();
        assert!(formatted.success());
        PartitionTarget {
            device: image.to_str().unwrap().to_owned(),
            number: 1,
            filesystem: Filesystem::Btrfs,
            size: 128 << 20,
            fs_uuid: None,
            fs_label: None,
        }
    }

    fn damage_tree_root(image: &str) -> Vec<(u64, Vec<u8>)> {
        use std::io::{Read as _, Seek as _, SeekFrom, Write as _};

        let run = |program: &str, args: &[&str]| {
            let output = std::process::Command::new(program)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{program}: {output:?}");
            String::from_utf8(output.stdout).unwrap()
        };
        let superblock = run("btrfs", &["inspect-internal", "dump-super", image]);
        let root = superblock
            .lines()
            .find_map(|line| line.strip_prefix("root\t"))
            .unwrap()
            .trim();
        let mut file = std::fs::File::options()
            .read(true)
            .write(true)
            .open(image)
            .unwrap();
        run("btrfs-map-logical", &["-l", root, image])
            .lines()
            .filter(|line| line.starts_with("mirror"))
            .map(|mirror| {
                let physical: u64 = mirror
                    .split_whitespace()
                    .skip_while(|&word| word != "physical")
                    .nth(1)
                    .unwrap()
                    .parse()
                    .unwrap();
                let offset = physical + 256;
                file.seek(SeekFrom::Start(offset)).unwrap();
                file.write_all(b"damaged tree block").unwrap();
                let mut written = vec![0; 4096];
                file.seek(SeekFrom::Start(offset)).unwrap();
                file.read_exact(&mut written).unwrap();
                (offset, written)
            })
            .collect()
    }

    #[tokio::test]
    async fn a_btrfs_partition_is_checked_read_only_and_one_with_errors_fails_the_capture() {
        use std::io::{Read as _, Seek as _, SeekFrom};

        let dir = std::env::temp_dir().join(format!("imaged-agent-btrfs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        repair_filesystem(&btrfs_target(&dir, "clean.img"))
            .await
            .expect("a clean btrfs filesystem passes the check");

        let damaged = btrfs_target(&dir, "damaged.img");
        let blocks = damage_tree_root(&damaged.device);
        let e = repair_filesystem(&damaged).await.unwrap_err();

        assert!(
            e.to_string()
                .starts_with(&format!("{} has filesystem errors", damaged.device)),
            "{e:#}"
        );
        let mut file = std::fs::File::open(&damaged.device).unwrap();
        for (offset, damage) in blocks {
            let mut now = vec![0; damage.len()];
            file.seek(SeekFrom::Start(offset)).unwrap();
            file.read_exact(&mut now).unwrap();
            assert_eq!(now, damage, "the check changed the block at {offset}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
