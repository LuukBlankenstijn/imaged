use crate::error::Error;
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

impl BlockDevice {
    pub fn get_device(&self) -> String {
        format!("/dev/{}", self.name)
    }

    pub fn find_partition_number(&self) -> Result<i64, Error> {
        let digits: String = self
            .name
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits
            .parse()
            .map_err(|e| Error::NoPartitionNumber(format!("{e}")))
    }

    pub fn get_partclone_binary(&self) -> Option<&'static str> {
        if let Some(filetype) = self.fstype.clone() {
            match filetype.as_str() {
                "ext2" | "ext3" | "ext4" => Some("partclone.extfs"),
                "vfat" | "fat32" | "fat16" => Some("partclone.vfat"),
                _ => None,
            }
        } else {
            None
        }
    }
}

#[derive(Debug, Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<BlockDevice>,
}

pub async fn find_target_disk() -> Result<BlockDevice, Error> {
    let devices = lsblk(None).await?;

    let candidates: Vec<&BlockDevice> = devices
        .iter()
        .filter(|d| !d.removable && d.transport.as_deref() != Some("usb"))
        .collect();

    let disk = match candidates.as_slice() {
        [] => return Err(Error::NoDisk),
        [d] => *d,
        many => {
            return Err(Error::MultipleDisks(
                many.iter().map(|d| d.name.clone()).collect(),
            ));
        }
    };

    if contains_root(disk) {
        return Err(Error::WouldImageRoot(disk.name.clone()));
    }

    Ok(disk.clone())
}

async fn lsblk(device: Option<&str>) -> Result<Vec<BlockDevice>, Error> {
    let mut cmd = Command::new("lsblk");
    cmd.args(["-J", "-b", "-o", "NAME,SIZE,FSTYPE,MOUNTPOINT,RM,TRAN"]);
    if let Some(d) = device {
        cmd.arg(d);
    }

    let output = cmd.output().await?;
    if !output.status.success() {
        return Err(Error::Lsblk(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(serde_json::from_slice::<LsblkOutput>(&output.stdout)?.blockdevices)
}

fn contains_root(dev: &BlockDevice) -> bool {
    dev.mountpoint.as_deref() == Some("/") || dev.children.iter().any(contains_root)
}
