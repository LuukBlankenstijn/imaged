use std::ffi::{OsStr, OsString};
use std::path::Path;

use anyhow::bail;
use tokio::process::Command;

pub async fn backup_partition_table(disk: &Path, backup: &Path) -> anyhow::Result<()> {
    sgdisk(
        [option("--backup=", backup).as_os_str(), disk.as_os_str()],
        "sgdisk failed",
    )
    .await
}

pub async fn restore_partition_table(disk: &Path, backup: &Path) -> anyhow::Result<()> {
    sgdisk(
        [OsStr::new("--zap-all"), disk.as_os_str()],
        "sgdisk --zap-all failed",
    )
    .await?;
    sgdisk(
        [
            option("--load-backup=", backup).as_os_str(),
            disk.as_os_str(),
        ],
        "sgdisk --load-backup failed",
    )
    .await?;
    let status = Command::new("partprobe")
        .kill_on_drop(true)
        .status()
        .await?;
    if !status.success() {
        bail!("partprobe failed");
    }
    Ok(())
}

fn option(name: &str, value: &Path) -> OsString {
    let mut option = OsString::from(name);
    option.push(value);
    option
}

async fn sgdisk(args: [&OsStr; 2], failure: &str) -> anyhow::Result<()> {
    let status = Command::new("sgdisk")
        .args(args)
        .kill_on_drop(true)
        .status()
        .await?;
    if !status.success() {
        bail!("{failure}");
    }
    Ok(())
}
