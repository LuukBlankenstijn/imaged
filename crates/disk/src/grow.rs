use std::ffi::OsStr;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use tokio::io::AsyncSeekExt as _;
use tokio::process::Command;
use tracing::{info, warn};

use crate::Filesystem;

const E2FSCK_CONFIG: &str = "/etc/e2fsck.conf";
const E2FSCK_UNCORRECTED: i32 = 4;

pub async fn grow(filesystem: Filesystem, device: &Path) -> anyhow::Result<()> {
    match filesystem {
        Filesystem::Ext2 | Filesystem::Ext3 | Filesystem::Ext4 => grow_ext(device).await,
        Filesystem::Btrfs => grow_btrfs(device).await,
        Filesystem::Vfat | Filesystem::Swap => Ok(()),
    }
}

async fn grow_ext(device: &Path) -> anyhow::Result<()> {
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

const BTRFS_MIN_RESIZE_BYTES: u64 = 256 * 1024 * 1024;

async fn grow_btrfs(device: &Path) -> anyhow::Result<()> {
    let available = partition_bytes(device).await?;
    let (total_bytes, sector_size) = btrfs_geometry(device).await?;
    if available / sector_size <= total_bytes / sector_size {
        return Ok(());
    }
    if available < BTRFS_MIN_RESIZE_BYTES {
        info!(
            device = %device.display(),
            partition_bytes = available,
            "not growing btrfs: the kernel refuses to resize it to less than 256 MiB"
        );
        return Ok(());
    }
    info!(
        device = %device.display(),
        filesystem_bytes = total_bytes,
        partition_bytes = available,
        "growing the filesystem to fill its partition"
    );
    let mounted = Mounted::new(device).await?;
    let resized = run(
        "btrfs",
        &[
            OsStr::new("filesystem"),
            OsStr::new("resize"),
            OsStr::new("max"),
            mounted.dir.as_os_str(),
        ],
    )
    .await;
    let unmounted = mounted.unmount().await;
    if let (Err(_), Err(e)) = (&resized, &unmounted) {
        warn!(device = %device.display(), error = %format!("{e:#}"), "could not unmount the filesystem after failing to grow it");
    }
    resized.and(unmounted)
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

async fn btrfs_geometry(device: &Path) -> anyhow::Result<(u64, u64)> {
    let output = Command::new("btrfs")
        .args(["inspect-internal", "dump-super"])
        .arg(device)
        .kill_on_drop(true)
        .output()
        .await
        .context("running btrfs inspect-internal dump-super")?;
    let superblock = String::from_utf8_lossy(&output.stdout);
    let field = |name: &str| {
        superblock
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(char::is_whitespace)?;
                (key == name).then(|| value.trim())
            })
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|&value| value > 0)
            .with_context(|| {
                format!(
                    "reading the {name} of the btrfs filesystem on {}: {}",
                    device.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )
            })
    };
    Ok((field("total_bytes")?, field("sectorsize")?))
}

struct Mounted {
    dir: PathBuf,
    mounted: bool,
}

impl Mounted {
    async fn new(device: &Path) -> anyhow::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("imaged-grow-")
            .tempdir()
            .context("creating a mount point")?
            .keep();
        let mounted = run(
            "mount",
            &[
                OsStr::new("-t"),
                OsStr::new("btrfs"),
                device.as_os_str(),
                dir.as_os_str(),
            ],
        )
        .await;
        if let Err(e) = mounted {
            let _ = tokio::fs::remove_dir(&dir).await;
            return Err(e);
        }
        Ok(Self { dir, mounted: true })
    }

    async fn unmount(mut self) -> anyhow::Result<()> {
        run("umount", &[self.dir.as_os_str()]).await?;
        self.mounted = false;
        tokio::fs::remove_dir(&self.dir)
            .await
            .with_context(|| format!("removing {}", self.dir.display()))
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        if !self.mounted {
            return;
        }
        let unmount = |args: &[&OsStr]| {
            std::process::Command::new("umount")
                .args(args)
                .output()
                .is_ok_and(|output| output.status.success())
        };
        if unmount(&[self.dir.as_os_str()]) || unmount(&[OsStr::new("-l"), self.dir.as_os_str()]) {
            let _ = std::fs::remove_dir(&self.dir);
        } else {
            warn!(dir = %self.dir.display(), "could not unmount the filesystem");
        }
    }
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

async fn run(program: &str, args: &[&OsStr]) -> anyhow::Result<()> {
    let output = Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("running {program}"))?;
    if output.status.success() {
        return Ok(());
    }
    let command: Vec<_> = std::iter::once(OsStr::new(program))
        .chain(args.iter().copied())
        .map(OsStr::to_string_lossy)
        .collect();
    bail!(
        "{} failed with {}: {}",
        command.join(" "),
        output.status,
        printed(output.stdout, output.stderr)
    )
}

fn printed(stdout: Vec<u8>, stderr: Vec<u8>) -> String {
    String::from_utf8_lossy(&[stdout, stderr].concat())
        .trim()
        .to_string()
}
