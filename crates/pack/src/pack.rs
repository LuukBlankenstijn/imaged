use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use chrono::Utc;
use imaged_archive::manifest::{PARTITION_TABLE_FILE, partition_file};
use imaged_archive::{Contents, Manifest, PartitionSource, write_archive};
use imaged_disk::{
    Filesystem, backup_partition_table, clone_compressed, holds_system, root_partition,
};
use tempfile::TempDir;
use tokio::fs::File;
use tracing::info;

use crate::cli::Args;
use crate::extract::extract;
use crate::layout::{self, Partition};
use crate::scripts;

const PARTCLONE_LOG: &str = "partclone.log";

struct Extracted {
    partition: Partition,
    filesystem: Filesystem,
    path: PathBuf,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let captured_at = Utc::now();
    let scripts = scripts::load(&args.scripts).await?;
    let layout = layout::partitions(&args.disk).await?;
    let work = TempDir::new().context("creating a temporary directory")?;

    let partition_table = work.path().join(PARTITION_TABLE_FILE);
    backup_partition_table(&args.disk, &partition_table)
        .await
        .context("backing up the partition table")?;

    let mut partitions = Vec::with_capacity(layout.len());
    let mut systems = Vec::new();
    for &partition in &layout {
        let Some(extracted) = extract_formatted(&args.disk, partition, work.path()).await? else {
            continue;
        };
        if args.root_partition.is_none()
            && holds_system(partition.number, extracted.filesystem, &extracted.path).await
        {
            systems.push(partition.number);
        }
        partitions.push(clone_partition(extracted, work.path()).await?);
    }
    let root_partition = choose_root(args.root_partition, &partitions, &systems)?;

    let contents = Contents {
        name: args.name,
        captured_at: Some(captured_at),
        partition_table,
        partitions,
        root_partition,
        scripts,
    };
    let manifest = write(contents, &args.output).await?;
    info!(
        output = %args.output.display(),
        partitions = manifest.partitions.len(),
        root_partition = ?manifest.root_partition,
        scripts = manifest.scripts.len(),
        "packed the disk image"
    );
    Ok(())
}

async fn extract_formatted(
    disk: &Path,
    partition: Partition,
    dir: &Path,
) -> anyhow::Result<Option<Extracted>> {
    let number = partition.number;
    let path = dir.join(format!("p-{number}.raw"));
    info!(
        partition = number,
        size_bytes = partition.size_bytes,
        "extracting partition"
    );
    extract(disk, partition, &path).await?;
    let fstype = layout::fstype(&path).await?;
    match Filesystem::detect(fstype.as_deref())
        .with_context(|| format!("cannot pack partition {number}"))?
    {
        Some(filesystem) => Ok(Some(Extracted {
            partition,
            filesystem,
            path,
        })),
        None => {
            info!(
                partition = number,
                "skipping partition without a filesystem"
            );
            remove(&path).await?;
            Ok(None)
        }
    }
}

fn choose_root(
    requested: Option<i64>,
    packed: &[PartitionSource],
    systems: &[i64],
) -> anyhow::Result<Option<i64>> {
    if let Some(root) = requested {
        if !packed.iter().any(|p| p.number == root) {
            let packed: Vec<i64> = packed.iter().map(|p| p.number).collect();
            bail!("root partition {root} is not one of the packed partitions {packed:?}");
        }
        return Ok(Some(root));
    }
    let root = root_partition(systems);
    if let Some(root) = root {
        info!(partition = root, "found the root partition");
    }
    Ok(root)
}

async fn clone_partition(extracted: Extracted, dir: &Path) -> anyhow::Result<PartitionSource> {
    let number = extracted.partition.number;
    let path = dir.join(partition_file(number));
    info!(partition = number, filesystem = %extracted.filesystem, "cloning partition");
    let failed = || format!("cloning partition {number}");
    let (partclone, mut compressed) = clone_compressed(
        extracted.filesystem,
        &extracted.path,
        &dir.join(PARTCLONE_LOG),
    )
    .with_context(failed)?;
    let mut file = File::create(&path)
        .await
        .with_context(|| format!("creating {}", path.display()))?;
    tokio::io::copy(&mut compressed, &mut file)
        .await
        .with_context(failed)?;
    partclone.wait().await.with_context(failed)?;
    remove(&extracted.path).await?;
    Ok(PartitionSource {
        number,
        fstype: extracted.filesystem.as_str().to_owned(),
        size_bytes: extracted.partition.size_bytes,
        path,
    })
}

async fn write(contents: Contents, output: &Path) -> anyhow::Result<Manifest> {
    let mut partial = output.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let written = write_partial(contents, &partial, output).await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
    }
    written
}

async fn write_partial(
    contents: Contents,
    partial: &Path,
    output: &Path,
) -> anyhow::Result<Manifest> {
    let mut file = File::create(partial)
        .await
        .with_context(|| format!("creating {}", partial.display()))?;
    let manifest = write_archive(contents, &mut file)
        .await
        .with_context(|| format!("writing {}", partial.display()))?;
    file.sync_all()
        .await
        .with_context(|| format!("writing {}", partial.display()))?;
    tokio::fs::rename(partial, output)
        .await
        .with_context(|| format!("moving {} to {}", partial.display(), output.display()))?;
    Ok(manifest)
}

async fn remove(path: &Path) -> anyhow::Result<()> {
    tokio::fs::remove_file(path)
        .await
        .with_context(|| format!("removing {}", path.display()))
}
