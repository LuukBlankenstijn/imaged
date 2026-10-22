use std::ffi::{OsStr, OsString};
use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::process::Command;
use tracing::info;

use crate::gpt::{Backup, Disk, Label};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionTable {
    pub sector_size: u32,
    pub partitions: Vec<Partition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub number: i64,
    pub offset: u64,
    pub size_bytes: u64,
}

pub async fn backup_partition_table(disk: &Path, backup: &Path) -> anyhow::Result<PartitionTable> {
    let mut source = Disk::open(disk).await?;
    let label = source
        .label()
        .await
        .with_context(|| format!("reading the partition table of {}", disk.display()))?;
    if label != Label::Gpt {
        bail!("only GPT disks are supported, found {label}");
    }
    run(
        "sgdisk",
        &[option("--backup=", backup).as_os_str(), disk.as_os_str()],
    )
    .await?;
    Ok(PartitionTable {
        sector_size: source.sector_size,
        partitions: read_backup(backup).await?.partitions(source.sector_size)?,
    })
}

pub async fn restore_partition_table(
    disk: &Path,
    backup: &Path,
    sector_size: u32,
) -> anyhow::Result<()> {
    let Disk {
        block_device,
        sector_size: target_sector_size,
        size_bytes,
        ..
    } = Disk::open(disk).await?;
    if target_sector_size != sector_size {
        bail!(
            "the image's partition table is for {sector_size}-byte sectors, {} has {target_sector_size}-byte sectors",
            disk.display()
        );
    }
    let required = read_backup(backup)
        .await?
        .required_sectors(sector_size)
        .saturating_mul(u64::from(sector_size));
    if size_bytes < required {
        bail!(
            "{} holds {size_bytes} bytes, the image's partitions need at least {required}",
            disk.display()
        );
    }
    run("sgdisk", &[OsStr::new("--zap-all"), disk.as_os_str()]).await?;
    run(
        "sgdisk",
        &[
            option("--load-backup=", backup).as_os_str(),
            disk.as_os_str(),
        ],
    )
    .await?;
    if block_device {
        run("partprobe", &[disk.as_os_str()]).await?;
    }
    Ok(())
}

async fn read_backup(backup: &Path) -> anyhow::Result<Backup> {
    let bytes = tokio::fs::read(backup)
        .await
        .with_context(|| format!("reading {}", backup.display()))?;
    Backup::parse(&bytes)
}

fn option(name: &str, value: &Path) -> OsString {
    let mut option = OsString::from(name);
    option.push(value);
    option
}

async fn run(program: &str, args: &[&OsStr]) -> anyhow::Result<()> {
    let output = Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await?;
    let printed =
        [output.stdout, output.stderr].map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let lines: Vec<&str> = printed
        .iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    for line in &lines {
        info!("{program}: {line}");
    }
    if !output.status.success() {
        let first = args
            .first()
            .map_or_else(String::new, |arg| format!(" {}", arg.display()));
        bail!(
            "{program}{first} failed with {}: {}",
            output.status,
            lines.join(" ")
        );
    }
    Ok(())
}
