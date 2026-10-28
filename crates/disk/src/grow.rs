use std::io::SeekFrom;
use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::io::AsyncSeekExt as _;
use tokio::process::Command;
use tracing::info;

use crate::Filesystem;

const E2FSCK_CONFIG: &str = "/etc/e2fsck.conf";
const E2FSCK_UNCORRECTED: i32 = 4;

pub async fn grow(filesystem: Filesystem, device: &Path) -> anyhow::Result<()> {
    if !filesystem.is_ext() {
        return Ok(());
    }
    let available = partition_bytes(device).await?;
    let (blocks, block_size) = ext_geometry(device).await?;
    if available / block_size <= blocks {
        return Ok(());
    }
    info!(
        device = %device.display(),
        filesystem_bytes = blocks * block_size,
        partition_bytes = available,
        "growing the filesystem to fill its partition"
    );
    check(device).await?;
    let output = Command::new("resize2fs")
        .arg("-f")
        .arg(device)
        .kill_on_drop(true)
        .output()
        .await
        .context("running resize2fs")?;
    if !output.status.success() {
        bail!(
            "resize2fs {} failed with {}: {}",
            device.display(),
            output.status,
            printed(output.stdout, output.stderr)
        );
    }
    Ok(())
}

async fn partition_bytes(device: &Path) -> anyhow::Result<u64> {
    let mut file = tokio::fs::File::open(device)
        .await
        .with_context(|| format!("opening {}", device.display()))?;
    file.seek(SeekFrom::End(0))
        .await
        .with_context(|| format!("measuring {}", device.display()))
}

async fn ext_geometry(device: &Path) -> anyhow::Result<(u64, u64)> {
    let output = Command::new("debugfs")
        .args(["-R", "stats"])
        .arg(device)
        .kill_on_drop(true)
        .output()
        .await
        .context("running debugfs")?;
    let stats = String::from_utf8_lossy(&output.stdout);
    let field = |name: &str| {
        stats
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|&value| value > 0)
            .with_context(|| {
                format!(
                    "reading the {} of the filesystem on {}: {}",
                    name.to_lowercase(),
                    device.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )
            })
    };
    Ok((field("Block count")?, field("Block size")?))
}

async fn check(device: &Path) -> anyhow::Result<()> {
    let output = Command::new("e2fsck")
        .env("E2FSCK_CONFIG", E2FSCK_CONFIG)
        .args(["-f", "-p"])
        .arg(device)
        .kill_on_drop(true)
        .output()
        .await
        .context("running e2fsck")?;
    if output
        .status
        .code()
        .is_some_and(|code| code < E2FSCK_UNCORRECTED)
    {
        return Ok(());
    }
    bail!(
        "{} has filesystem errors that could not be repaired before growing it ({}): {}",
        device.display(),
        output.status,
        printed(output.stdout, output.stderr)
    )
}

fn printed(stdout: Vec<u8>, stderr: Vec<u8>) -> String {
    String::from_utf8_lossy(&[stdout, stderr].concat())
        .trim()
        .to_string()
}
