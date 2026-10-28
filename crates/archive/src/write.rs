use std::io::{self, Seek};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use imaged_model::error::{AppError, Result};
use imaged_model::image::ImageKind;
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufWriter};
use tokio_tar::{Builder, EntryType, Header};

use crate::manifest::{
    ArchivedFile, ArchivedPartition, ArchivedScript, MANIFEST_FILE, Manifest, PARTITION_TABLE_FILE,
    partition_file, script_file,
};
use crate::{CHUNK_BYTES, Script};

pub struct PartitionSource {
    pub number: i64,
    pub fstype: String,
    pub size_bytes: u64,
    pub fs_uuid: Option<String>,
    pub fs_label: Option<String>,
    pub path: PathBuf,
}

pub struct Contents {
    pub name: String,
    pub captured_at: Option<DateTime<Utc>>,
    pub kind: ImageKind,
    pub sector_size: u32,
    pub partition_table: Option<PathBuf>,
    pub partitions: Vec<PartitionSource>,
    pub root_partition: Option<i64>,
    pub scripts: Vec<Script>,
}

struct HashedFile {
    file: File,
    len: u64,
    sha256: String,
}

impl HashedFile {
    async fn open(path: PathBuf) -> Result<Self> {
        tokio::task::spawn_blocking(move || {
            Self::hash(&path)
                .map_err(|e| AppError::Internal(format!("failed to read {}: {e}", path.display())))
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    }

    fn hash(path: &Path) -> io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let len = io::copy(&mut file, &mut hasher)?;
        file.rewind()?;
        Ok(Self {
            file: File::from_std(file),
            len,
            sha256: format!("{:x}", hasher.finalize()),
        })
    }
}

pub async fn write_archive<W>(contents: Contents, writer: W) -> Result<Manifest>
where
    W: AsyncWrite + Unpin + Send,
{
    let partition_table = match contents.partition_table {
        Some(path) => Some(HashedFile::open(path).await?),
        None => None,
    };
    let mut partition_files = Vec::with_capacity(contents.partitions.len());
    for partition in &contents.partitions {
        partition_files.push(HashedFile::open(partition.path.clone()).await?);
    }

    let mut manifest = Manifest {
        format: 0,
        name: contents.name,
        kind: contents.kind,
        captured_at: contents.captured_at,
        sector_size: contents.sector_size,
        partition_table: partition_table.as_ref().map(|table| ArchivedFile {
            file: PARTITION_TABLE_FILE.to_string(),
            sha256: table.sha256.clone(),
        }),
        partitions: contents
            .partitions
            .iter()
            .zip(&partition_files)
            .map(|(partition, hashed)| ArchivedPartition {
                number: partition.number,
                fstype: partition.fstype.clone(),
                size_bytes: partition.size_bytes,
                file: partition_file(partition.number),
                sha256: hashed.sha256.clone(),
                fs_uuid: partition.fs_uuid.clone(),
                fs_label: partition.fs_label.clone(),
            })
            .collect(),
        root_partition: contents.root_partition,
        scripts: contents
            .scripts
            .iter()
            .map(|script| ArchivedScript {
                file: script_file(&script.name),
                sha256: format!("{:x}", Sha256::digest(&script.body)),
                name: script.name.clone(),
                inputs: script.inputs.clone(),
                root_partition: script.root_partition,
            })
            .collect(),
    };
    manifest.format = manifest.required_format();
    manifest.checksums()?;
    let manifest_json =
        serde_json::to_vec_pretty(&manifest).map_err(|e| AppError::Internal(e.to_string()))?;

    let mut builder = Builder::new_non_terminated(BufWriter::with_capacity(CHUNK_BYTES, writer));
    append(
        &mut builder,
        MANIFEST_FILE,
        manifest_json.len() as u64,
        manifest_json.as_slice(),
    )
    .await?;
    if let (Some(archived), Some(table)) = (&manifest.partition_table, partition_table) {
        append(&mut builder, &archived.file, table.len, table.file).await?;
    }
    for (archived, hashed) in manifest.partitions.iter().zip(partition_files) {
        append(&mut builder, &archived.file, hashed.len, hashed.file).await?;
    }
    for (archived, script) in manifest.scripts.iter().zip(&contents.scripts) {
        let body = script.body.as_bytes();
        append(&mut builder, &archived.file, body.len() as u64, body).await?;
    }
    let mut writer = builder.into_inner().await.map_err(write_failed)?;
    writer.shutdown().await.map_err(write_failed)?;
    Ok(manifest)
}

async fn append<W, R>(builder: &mut Builder<W>, name: &str, len: u64, data: R) -> Result
where
    W: AsyncWrite + Unpin + Send,
    R: AsyncRead + Unpin,
{
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(len);
    builder
        .append_data(&mut header, name, data)
        .await
        .map_err(write_failed)
}

fn write_failed(error: io::Error) -> AppError {
    AppError::Internal(format!("failed to write archive: {error}"))
}
