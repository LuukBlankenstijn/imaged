use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Component, Path};

use futures::StreamExt;
use futures::stream::BoxStream;
use imaged_model::error::{AppError, Result};
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_tar::{Archive, Entry};

use crate::manifest::{
    MANIFEST_FILE, MAX_MANIFEST_BYTES, MAX_PARTITION_TABLE_BYTES, MAX_SCRIPT_BYTES, Manifest,
    PARTITION_TABLE_FILE, SCRIPTS_DIR, is_script_file,
};
use crate::{CHUNK_BYTES, Script};

type Entries<'a, R> = BoxStream<'a, io::Result<Entry<Archive<R>>>>;

pub struct ArchiveReader<'a, R: AsyncRead + Unpin> {
    archive: Archive<R>,
    entries: Entries<'a, R>,
    manifest: Manifest,
}

pub struct Unpacked {
    pub manifest: Manifest,
    pub scripts: Vec<Script>,
}

impl<'a, R: AsyncRead + Unpin + Send + 'a> ArchiveReader<'a, R> {
    pub async fn open(reader: R) -> Result<Self> {
        let mut archive = Archive::new(reader);
        let mut entries = archive.entries_raw().map_err(malformed)?.boxed();
        let manifest = read_manifest(&mut entries).await?;
        manifest.checksums()?;
        Ok(Self {
            archive,
            entries,
            manifest,
        })
    }
}

impl<R: AsyncRead + Unpin> ArchiveReader<'_, R> {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub async fn unpack(self, dir: &Path) -> Result<Unpacked> {
        let Self {
            archive,
            mut entries,
            manifest,
        } = self;
        tokio::fs::create_dir_all(dir).await.map_err(store_failed)?;
        let mut bodies = receive(&mut entries, Some(dir), manifest.checksums()?).await?;
        drop(entries);
        read_to_end(archive).await?;
        let scripts = manifest
            .scripts
            .iter()
            .map(|archived| {
                Ok(Script {
                    name: archived.name.clone(),
                    body: bodies.remove(&archived.file).ok_or_else(|| {
                        AppError::InvalidArgument(format!("archive is missing `{}`", archived.file))
                    })?,
                    inputs: archived.inputs.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Unpacked { manifest, scripts })
    }

    pub async fn verify(self) -> Result<Manifest> {
        let Self {
            archive,
            mut entries,
            manifest,
        } = self;
        receive(&mut entries, None, manifest.checksums()?).await?;
        drop(entries);
        read_to_end(archive).await?;
        Ok(manifest)
    }
}

async fn read_manifest<R>(entries: &mut Entries<'_, R>) -> Result<Manifest>
where
    R: AsyncRead + Unpin,
{
    let mut entry = entries
        .next()
        .await
        .ok_or_else(|| AppError::InvalidArgument("archive is empty".to_string()))?
        .map_err(malformed)?;
    let name = entry_name(&entry)?;
    if name != MANIFEST_FILE {
        return Err(AppError::InvalidArgument(format!(
            "archive must start with {MANIFEST_FILE}, found `{name}`"
        )));
    }

    let mut json = Vec::new();
    (&mut entry)
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut json)
        .await
        .map_err(malformed)?;
    Manifest::parse(&json)
}

async fn read_to_end<R>(archive: Archive<R>) -> Result
where
    R: AsyncRead + Unpin,
{
    let Ok(mut reader) = archive.into_inner() else {
        return Err(AppError::Internal(
            "archive reader is still in use after the last entry".to_string(),
        ));
    };
    tokio::io::copy(&mut reader, &mut tokio::io::sink())
        .await
        .map_err(malformed)?;
    Ok(())
}

async fn receive<R>(
    entries: &mut Entries<'_, R>,
    dir: Option<&Path>,
    checksums: BTreeMap<&str, &str>,
) -> Result<BTreeMap<String, String>>
where
    R: AsyncRead + Unpin,
{
    let mut received = HashSet::new();
    let mut scripts = BTreeMap::new();
    while let Some(entry) = entries.next().await {
        let mut entry = entry.map_err(malformed)?;
        let name = entry_name(&entry)?;
        let Some(expected) = checksums.get(name.as_str()) else {
            return Err(AppError::InvalidArgument(format!(
                "unexpected archive entry `{name}`"
            )));
        };
        if !received.insert(name.clone()) {
            return Err(AppError::InvalidArgument(format!(
                "duplicate archive entry `{name}`"
            )));
        }
        if name == PARTITION_TABLE_FILE && entry.effective_size() > MAX_PARTITION_TABLE_BYTES {
            return Err(AppError::InvalidArgument(format!(
                "{PARTITION_TABLE_FILE} exceeds {MAX_PARTITION_TABLE_BYTES} bytes"
            )));
        }
        let actual = if is_script_file(&name) {
            if entry.effective_size() > MAX_SCRIPT_BYTES {
                return Err(AppError::InvalidArgument(format!(
                    "{name} exceeds {MAX_SCRIPT_BYTES} bytes"
                )));
            }
            let mut body = String::new();
            (&mut entry)
                .take(MAX_SCRIPT_BYTES)
                .read_to_string(&mut body)
                .await
                .map_err(malformed)?;
            let sha256 = format!("{:x}", Sha256::digest(&body));
            scripts.insert(name.clone(), body);
            sha256
        } else {
            write_hashed(&mut entry, dir.map(|dir| dir.join(&name)).as_deref()).await?
        };
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(AppError::InvalidArgument(format!(
                "checksum mismatch for `{name}`"
            )));
        }
    }
    match checksums.keys().find(|file| !received.contains(**file)) {
        Some(missing) => Err(AppError::InvalidArgument(format!(
            "archive is missing `{missing}`"
        ))),
        None => Ok(scripts),
    }
}

fn entry_name<R>(entry: &Entry<R>) -> Result<String>
where
    R: AsyncRead + Unpin,
{
    let path = entry.path().map_err(malformed)?;
    let mut components = path.components();
    let plain_name = match (components.next(), components.next(), components.next()) {
        _ if !entry.header().entry_type().is_file() => None,
        (Some(Component::Normal(name)), None, None) => name.to_str().map(str::to_owned),
        (Some(Component::Normal(dir)), Some(Component::Normal(name)), None)
            if dir == SCRIPTS_DIR =>
        {
            name.to_str().map(|name| format!("{SCRIPTS_DIR}/{name}"))
        }
        _ => None,
    };
    plain_name.ok_or_else(|| {
        AppError::InvalidArgument(format!(
            "archive entry `{}` is not a plain file",
            path.display()
        ))
    })
}

async fn write_hashed<R>(data: &mut R, path: Option<&Path>) -> Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut file = match path {
        Some(path) => Some(File::create(path).await.map_err(store_failed)?),
        None => None,
    };
    let mut hasher = Sha256::new();
    let mut chunk = vec![0; CHUNK_BYTES];
    loop {
        let read = data.read(&mut chunk).await.map_err(malformed)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        if let Some(file) = &mut file {
            file.write_all(&chunk[..read]).await.map_err(store_failed)?;
        }
    }
    if let Some(file) = &mut file {
        file.flush().await.map_err(store_failed)?;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn malformed(error: io::Error) -> AppError {
    AppError::InvalidArgument(format!("malformed archive: {error}"))
}

fn store_failed(error: io::Error) -> AppError {
    AppError::Internal(format!("failed to store imported image: {error}"))
}
