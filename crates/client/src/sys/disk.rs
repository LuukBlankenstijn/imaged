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
        let digits: String = self
            .name
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits
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
}
