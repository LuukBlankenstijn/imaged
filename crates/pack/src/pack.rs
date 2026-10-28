use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use chrono::Utc;
use imaged_archive::manifest::{PARTITION_TABLE_FILE, partition_file};
use imaged_archive::{Contents, Manifest, PartitionSource, Script, write_archive};
use imaged_disk::{
    Filesystem, Partition, backup_partition_table, clone_compressed, holds_system, root_partition,
};
use imaged_model::image::ImageKind;
use tempfile::TempDir;
use tokio::fs::File;
use tokio::io::AsyncSeekExt as _;
use tracing::info;

use crate::blkid::{self, Probe};
use crate::cli::Args;
use crate::extract::extract;
use crate::scripts;
use crate::shrink::shrink_ext;

const PARTCLONE_LOG: &str = "partclone.log";
const PARTITION_SECTOR_SIZE: u32 = 512;
const GPT: &str = "gpt";

struct Extracted {
    filesystem: Filesystem,
    fs_uuid: Option<String>,
    fs_label: Option<String>,
    path: PathBuf,
}

struct Packed {
    kind: ImageKind,
    sector_size: u32,
    partition_table: Option<PathBuf>,
    partitions: Vec<PartitionSource>,
    systems: Vec<i64>,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let captured_at = Utc::now();
    let scripts = scripts::load(&args.scripts, &args.script_roots).await?;
    let work = TempDir::new().context("creating a temporary directory")?;

    let probe = blkid::probe(&args.image).await?;
    let packed = match probe {
        Probe {
            fstype: Some(fstype),
            uuid,
            label,
            pttype,
        } if pttype.as_deref() != Some(GPT) => {
            pack_partition(&args, &fstype, uuid, label, work.path()).await?
        }
        _ => pack_disk(&args, work.path()).await?,
    };
    let root_partition = choose_root(args.root_partition, &packed.partitions, &packed.systems)?;
    check_script_roots(&scripts, &packed.partitions)?;

    let contents = Contents {
        name: args.name,
        captured_at: Some(captured_at),
        kind: packed.kind,
        sector_size: packed.sector_size,
        partition_table: packed.partition_table,
        partitions: packed.partitions,
        root_partition,
        scripts,
    };
    let manifest = write(contents, &args.output).await?;
    info!(
        output = %args.output.display(),
        kind = %manifest.kind,
        partitions = manifest.partitions.len(),
        root_partition = ?manifest.root_partition,
        scripts = manifest.scripts.len(),
        "packed the image"
    );
    Ok(())
}

async fn pack_disk(args: &Args, dir: &Path) -> anyhow::Result<Packed> {
    let partition_table = dir.join(PARTITION_TABLE_FILE);
    let table = backup_partition_table(&args.image, &partition_table)
        .await
        .context("backing up the partition table")?;

    let mut partitions = Vec::with_capacity(table.partitions.len());
    let mut systems = Vec::new();
    for &partition in &table.partitions {
        let Some(extracted) = extract_formatted(&args.image, partition, dir).await? else {
            continue;
        };
        let number = partition.number;
        if args.root_partition.is_none()
            && holds_system(number, extracted.filesystem, &extracted.path).await
        {
            systems.push(number);
        }
        let path = clone_partition(number, extracted.filesystem, &extracted.path, dir).await?;
        remove(&extracted.path).await?;
        partitions.push(PartitionSource {
            number,
            fstype: extracted.filesystem.as_str().to_owned(),
            size_bytes: partition.size_bytes,
            fs_uuid: extracted.fs_uuid,
            fs_label: extracted.fs_label,
            path,
        });
    }
    Ok(Packed {
        kind: ImageKind::Disk,
        sector_size: table.sector_size,
        partition_table: Some(partition_table),
        partitions,
        systems,
    })
}

async fn pack_partition(
    args: &Args,
    fstype: &str,
    fs_uuid: Option<String>,
    fs_label: Option<String>,
    dir: &Path,
) -> anyhow::Result<Packed> {
    let supported = ImageKind::Partition.fstypes();
    let filesystem = Filesystem::detect(Some(fstype))
        .ok()
        .flatten()
        .filter(|filesystem| supported.contains(&filesystem.as_str()))
        .with_context(|| {
            format!(
                "{} holds neither a GPT partition table nor a supported filesystem: found {fstype}, expected one of {}",
                args.image.display(),
                supported.join(", ")
            )
        })?;
    let number = 1;
    let image_size = file_size(&args.image).await?;
    info!(filesystem = %filesystem, size_bytes = image_size, "packing a filesystem image as a partition image");

    let (source, size_bytes) = if filesystem.is_ext() {
        let copy = dir.join(format!("p-{number}.raw"));
        let whole = Partition {
            number,
            offset: 0,
            size_bytes: image_size,
        };
        extract(&args.image, whole, &copy).await?;
        let size_bytes = shrink_ext(&copy).await?;
        (copy, size_bytes)
    } else {
        (args.image.clone(), image_size)
    };

    let mut systems = Vec::new();
    if args.root_partition.is_none() && holds_system(number, filesystem, &source).await {
        systems.push(number);
    }
    let path = clone_partition(number, filesystem, &source, dir).await?;
    if source != args.image {
        remove(&source).await?;
    }
    Ok(Packed {
        kind: ImageKind::Partition,
        sector_size: PARTITION_SECTOR_SIZE,
        partition_table: None,
        partitions: vec![PartitionSource {
            number,
            fstype: filesystem.as_str().to_owned(),
            size_bytes,
            fs_uuid,
            fs_label,
            path,
        }],
        systems,
    })
}

async fn file_size(path: &Path) -> anyhow::Result<u64> {
    let failed = || format!("inspecting {}", path.display());
    let mut file = File::open(path).await.with_context(failed)?;
    file.seek(SeekFrom::End(0)).await.with_context(failed)
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
    let probe = blkid::probe(&path).await?;
    match Filesystem::detect(probe.fstype.as_deref())
        .with_context(|| format!("cannot pack partition {number}"))?
    {
        Some(filesystem) => Ok(Some(Extracted {
            filesystem,
            fs_uuid: probe.uuid,
            fs_label: probe.label,
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

fn packed_numbers(packed: &[PartitionSource]) -> Vec<i64> {
    packed.iter().map(|p| p.number).collect()
}

fn choose_root(
    requested: Option<i64>,
    packed: &[PartitionSource],
    systems: &[i64],
) -> anyhow::Result<Option<i64>> {
    if let Some(root) = requested {
        if !packed.iter().any(|p| p.number == root) {
            let packed = packed_numbers(packed);
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

fn check_script_roots(scripts: &[Script], packed: &[PartitionSource]) -> anyhow::Result<()> {
    for script in scripts {
        if let Some(root) = script.root_partition
            && !packed.iter().any(|p| p.number == root)
        {
            let packed = packed_numbers(packed);
            bail!(
                "script `{}` root partition {root} is not one of the packed partitions {packed:?}",
                script.name
            );
        }
    }
    Ok(())
}

async fn clone_partition(
    number: i64,
    filesystem: Filesystem,
    source: &Path,
    dir: &Path,
) -> anyhow::Result<PathBuf> {
    let path = dir.join(partition_file(number));
    info!(partition = number, filesystem = %filesystem, "cloning partition");
    let failed = || format!("cloning partition {number}");
    let mut compressed = clone_compressed(filesystem, source, &dir.join(PARTCLONE_LOG), |_| {})
        .with_context(failed)?;
    let mut file = File::create(&path)
        .await
        .with_context(|| format!("creating {}", path.display()))?;
    tokio::io::copy(&mut compressed, &mut file)
        .await
        .with_context(failed)?;
    Ok(path)
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
