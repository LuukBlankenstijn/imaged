use std::collections::{BTreeMap, HashSet};
use std::io::{self, Seek};
use std::path::{Component, Path};
use std::sync::Arc;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufWriter};
use tokio_tar::{Archive, Builder, Entry, EntryType, Header};
use tokio_util::io::ReaderStream;

use crate::domain::image::{Image, ImageRepository, NewImagePartition};
use crate::error::{AppError, Result};
use crate::service::image::ImageService;

const FORMAT: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";
const PARTITION_TABLE_FILE: &str = "parttable.bin";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_PARTITION_TABLE_BYTES: u64 = 1024 * 1024;
const INTERRUPTED_IMPORT_ERROR: &str = "import interrupted";
const CHUNK_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
struct FormatProbe {
    format: u32,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    name: String,
    captured_at: Option<DateTime<Utc>>,
    partition_table: ArchivedFile,
    partitions: Vec<ArchivedPartition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_partition: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct ArchivedFile {
    file: String,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
struct ArchivedPartition {
    number: i64,
    fstype: String,
    size_bytes: u64,
    file: String,
    sha256: String,
}

impl Manifest {
    fn checksums(&self) -> Result<BTreeMap<&str, &str>> {
        if self.partition_table.file != PARTITION_TABLE_FILE {
            return Err(AppError::InvalidArgument(format!(
                "partition table must be stored as {PARTITION_TABLE_FILE}, not `{}`",
                self.partition_table.file
            )));
        }
        let mut checksums =
            BTreeMap::from([(PARTITION_TABLE_FILE, self.partition_table.sha256.as_str())]);
        for partition in &self.partitions {
            if partition.number < 1 {
                return Err(AppError::InvalidArgument(format!(
                    "invalid partition number {}",
                    partition.number
                )));
            }
            let expected_file = partition_file(partition.number);
            if partition.file != expected_file {
                return Err(AppError::InvalidArgument(format!(
                    "partition {} must be stored as {expected_file}, not `{}`",
                    partition.number, partition.file
                )));
            }
            if checksums
                .insert(&partition.file, &partition.sha256)
                .is_some()
            {
                return Err(AppError::InvalidArgument(format!(
                    "partition {} is listed twice",
                    partition.number
                )));
            }
        }
        if let Some(root) = self.root_partition
            && !self.partitions.iter().any(|p| p.number == root)
        {
            return Err(AppError::InvalidArgument(format!(
                "root partition {root} is not one of the archived partitions"
            )));
        }
        Ok(checksums)
    }

    fn partitions(&self) -> Vec<NewImagePartition> {
        self.partitions
            .iter()
            .map(|partition| NewImagePartition {
                partition_number: partition.number,
                fstype: partition.fstype.clone(),
                size_bytes: partition.size_bytes,
            })
            .collect()
    }
}

struct HashedFile {
    file: File,
    len: u64,
    sha256: String,
}

impl HashedFile {
    async fn open(path: String) -> Result<Self> {
        tokio::task::spawn_blocking(move || {
            Self::hash(&path).map_err(|e| AppError::Internal(format!("failed to read {path}: {e}")))
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    }

    fn hash(path: &str) -> io::Result<Self> {
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

pub struct ArchiveService {
    image_repo: Arc<dyn ImageRepository>,
    image_service: Arc<ImageService>,
}

impl ArchiveService {
    pub async fn start(
        image_repo: Arc<dyn ImageRepository>,
        image_service: Arc<ImageService>,
    ) -> Result<Self> {
        image_repo
            .fault_unfinished_imports(INTERRUPTED_IMPORT_ERROR)
            .await?;
        image_service.clear_import_dirs().await?;
        Ok(Self {
            image_repo,
            image_service,
        })
    }

    pub async fn exportable(&self, image_id: i64) -> Result<Image> {
        let image = self.image_repo.get(image_id).await?;
        if !image.status.is_ready() {
            return Err(AppError::FailedPrecondition(format!(
                "image {image_id} is {}, only ready images can be exported",
                image.status
            )));
        }
        Ok(image)
    }

    pub async fn export<W>(&self, image_id: i64, writer: W) -> Result
    where
        W: AsyncWrite + Unpin + Send,
    {
        let image = self.exportable(image_id).await?;
        let partition_table =
            HashedFile::open(self.image_service.get_partition_table_path(image_id)).await?;
        let mut partition_files = Vec::with_capacity(image.partitions.len());
        for partition in &image.partitions {
            let path = self
                .image_service
                .get_partition_path(image_id, partition.partition_number);
            partition_files.push(HashedFile::open(path).await?);
        }

        let manifest = Manifest {
            format: FORMAT,
            name: image.name,
            captured_at: image.captured_at,
            partition_table: ArchivedFile {
                file: PARTITION_TABLE_FILE.to_string(),
                sha256: partition_table.sha256.clone(),
            },
            partitions: image
                .partitions
                .iter()
                .zip(&partition_files)
                .map(|(partition, hashed)| ArchivedPartition {
                    number: partition.partition_number,
                    fstype: partition.fstype.clone(),
                    size_bytes: partition.size_bytes,
                    file: partition_file(partition.partition_number),
                    sha256: hashed.sha256.clone(),
                })
                .collect(),
            root_partition: image.root_partition,
        };
        let manifest_json =
            serde_json::to_vec_pretty(&manifest).map_err(|e| AppError::Internal(e.to_string()))?;

        let mut builder =
            Builder::new_non_terminated(BufWriter::with_capacity(CHUNK_BYTES, writer));
        append(
            &mut builder,
            MANIFEST_FILE,
            manifest_json.len() as u64,
            manifest_json.as_slice(),
        )
        .await?;
        append(
            &mut builder,
            &manifest.partition_table.file,
            partition_table.len,
            partition_table.file,
        )
        .await?;
        for (archived, hashed) in manifest.partitions.iter().zip(partition_files) {
            append(&mut builder, &archived.file, hashed.len, hashed.file).await?;
        }
        let mut writer = builder.into_inner().await.map_err(write_failed)?;
        writer.shutdown().await.map_err(write_failed)
    }

    pub fn export_stream(
        self: &Arc<Self>,
        image_id: i64,
    ) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
        let (reader, writer) = tokio::io::duplex(CHUNK_BYTES);
        let service = Arc::clone(self);
        let export = futures::stream::once(async move { service.export(image_id, writer).await })
            .filter_map(move |exported| async move {
                let error = exported.err()?;
                tracing::warn!(image_id, "image export failed: {error}");
                Some(Err(io::Error::other(error)))
            });
        futures::stream::select(ReaderStream::with_capacity(reader, CHUNK_BYTES), export)
    }

    pub async fn import<R>(&self, reader: R) -> Result<Image>
    where
        R: AsyncRead + Unpin + Send,
    {
        let mut archive = Archive::new(reader);
        let mut entries = archive.entries_raw().map_err(malformed)?;
        let manifest = read_manifest(&mut entries).await?;
        let checksums = manifest.checksums()?;
        let image = self.image_repo.start_import(manifest.name.clone()).await?;

        match self
            .receive(image.id, &manifest, checksums, &mut entries)
            .await
        {
            Ok(imported) => Ok(imported),
            Err(error) => {
                self.discard(image.id, &error).await;
                Err(error)
            }
        }
    }

    async fn receive<A, S>(
        &self,
        image_id: i64,
        manifest: &Manifest,
        checksums: BTreeMap<&str, &str>,
        entries: &mut S,
    ) -> Result<Image>
    where
        A: AsyncRead + Unpin,
        S: Stream<Item = io::Result<Entry<A>>> + Unpin,
    {
        let import_dir = self.image_service.import_dir(image_id);
        tokio::fs::create_dir_all(&import_dir)
            .await
            .map_err(store_failed)?;
        unpack(entries, Path::new(&import_dir), checksums).await?;
        tokio::fs::rename(&import_dir, self.image_service.image_dir(image_id))
            .await
            .map_err(store_failed)?;
        self.image_repo
            .finish_import(
                image_id,
                manifest.captured_at,
                &manifest.partitions(),
                manifest.root_partition,
            )
            .await
    }

    async fn discard(&self, image_id: i64, error: &AppError) {
        if let Err(e) = self
            .image_repo
            .mark_faulted(image_id, &error.to_string())
            .await
        {
            tracing::error!(image_id, "failed to mark rejected import faulted: {e}");
        }
        if let Err(e) = self.image_service.clear_image_data(image_id).await {
            tracing::error!(image_id, "failed to remove rejected import: {e}");
        }
    }
}

pub fn archive_file_name(image_name: &str) -> String {
    let stem: String = image_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() {
        "image.imaged".to_string()
    } else {
        format!("{stem}.imaged")
    }
}

fn partition_file(number: i64) -> String {
    format!("p-{number}.pcl")
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

async fn read_manifest<A, S>(entries: &mut S) -> Result<Manifest>
where
    A: AsyncRead + Unpin,
    S: Stream<Item = io::Result<Entry<A>>> + Unpin,
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
    if json.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(AppError::InvalidArgument(format!(
            "{MANIFEST_FILE} exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }

    let probe: FormatProbe = serde_json::from_slice(&json).map_err(invalid_manifest)?;
    if probe.format != FORMAT {
        return Err(AppError::InvalidArgument(format!(
            "unsupported archive format {}, expected {FORMAT}",
            probe.format
        )));
    }
    serde_json::from_slice(&json).map_err(invalid_manifest)
}

async fn unpack<A, S>(entries: &mut S, dir: &Path, checksums: BTreeMap<&str, &str>) -> Result
where
    A: AsyncRead + Unpin,
    S: Stream<Item = io::Result<Entry<A>>> + Unpin,
{
    let mut received = HashSet::new();
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
        let actual = write_hashed(&mut entry, &dir.join(&name)).await?;
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
        None => Ok(()),
    }
}

fn entry_name<R>(entry: &Entry<R>) -> Result<String>
where
    R: AsyncRead + Unpin,
{
    let path = entry.path().map_err(malformed)?;
    let mut components = path.components();
    let plain_name = match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) if entry.header().entry_type().is_file() => {
            name.to_str()
        }
        _ => None,
    };
    plain_name.map(str::to_owned).ok_or_else(|| {
        AppError::InvalidArgument(format!(
            "archive entry `{}` is not a plain file",
            path.display()
        ))
    })
}

async fn write_hashed<R>(data: &mut R, path: &Path) -> Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut file = File::create(path).await.map_err(store_failed)?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0; CHUNK_BYTES];
    loop {
        let read = data.read(&mut chunk).await.map_err(malformed)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        file.write_all(&chunk[..read]).await.map_err(store_failed)?;
    }
    file.flush().await.map_err(store_failed)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn malformed(error: io::Error) -> AppError {
    AppError::InvalidArgument(format!("malformed archive: {error}"))
}

fn invalid_manifest(error: serde_json::Error) -> AppError {
    AppError::InvalidArgument(format!("invalid {MANIFEST_FILE}: {error}"))
}

fn store_failed(error: io::Error) -> AppError {
    AppError::Internal(format!("failed to store imported image: {error}"))
}

fn write_failed(error: io::Error) -> AppError {
    AppError::Internal(format!("failed to write archive: {error}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::*;
    use crate::di::DIContainer;
    use crate::domain::image::ImageStatus;

    static DB_ID: AtomicU64 = AtomicU64::new(0);

    const TABLE: &[u8] = b"partition table";
    const SMALL: &[u8] = b"second-partition-contents";

    struct TestDir(PathBuf);

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn container() -> (DIContainer, TestDir) {
        let id = DB_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("imaged-archive-{}-{}", std::process::id(), id));
        let c = crate::build_test_container(&dir).await;
        (c, TestDir(dir))
    }

    fn large() -> Vec<u8> {
        (0..600_000u32).map(|i| (i % 251) as u8).collect()
    }

    fn sha256(data: &[u8]) -> String {
        format!("{:x}", Sha256::digest(data))
    }

    async fn ready_image(c: &DIContainer) -> Image {
        let image = c.image_repo.create_image("golden".into()).await.unwrap();
        c.image_service
            .save_partition_table(image.id, TABLE)
            .await
            .unwrap();
        for (number, fstype, data) in [(1, "vfat", large()), (2, "ntfs", SMALL.to_vec())] {
            c.image_service
                .save_partition_data(
                    image.id,
                    number,
                    futures::stream::iter([Ok(Bytes::from(data))]),
                )
                .await
                .unwrap();
            c.image_repo
                .save_partition(image.id, number, fstype, 1 << 33)
                .await
                .unwrap();
        }
        c.image_repo.mark_finished(image.id).await.unwrap();
        c.image_repo.get(image.id).await.unwrap()
    }

    async fn export(c: &DIContainer, image_id: i64) -> Vec<u8> {
        let mut archive = Vec::new();
        c.archive_service
            .export(image_id, &mut archive)
            .await
            .unwrap();
        archive
    }

    fn manifest(partitions: &[(i64, &[u8])]) -> Vec<u8> {
        manifest_with_table(TABLE, partitions)
    }

    fn manifest_with_table(table: &[u8], partitions: &[(i64, &[u8])]) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "format": 1,
            "name": "imported",
            "captured_at": null,
            "partition_table": { "file": "parttable.bin", "sha256": sha256(table) },
            "partitions": partitions
                .iter()
                .map(|(number, data)| json!({
                    "number": number,
                    "fstype": "ext4",
                    "size_bytes": 4096,
                    "file": format!("p-{number}.pcl"),
                    "sha256": sha256(data),
                }))
                .collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    async fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let regular: Vec<(&str, EntryType, &[u8])> = entries
            .iter()
            .map(|&(name, data)| (name, EntryType::Regular, data))
            .collect();
        typed_tar(&regular).await
    }

    async fn typed_tar(entries: &[(&str, EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = Builder::new_non_terminated(Vec::new());
        for &(name, entry_type, data) in entries {
            let mut header = Header::new_gnu();
            header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_entry_type(entry_type);
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append(&header, data).await.unwrap();
        }
        builder.into_inner().await.unwrap()
    }

    async fn untar(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut archive = Archive::new(archive);
        let mut entries = archive.entries().unwrap();
        let mut files = Vec::new();
        while let Some(entry) = entries.next().await {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).await.unwrap();
            files.push((name, data));
        }
        files
    }

    async fn rejected_import(c: &DIContainer, archive: &[u8]) -> String {
        match c.archive_service.import(archive).await {
            Err(AppError::InvalidArgument(message)) => message,
            other => panic!("expected an invalid argument error, got {other:?}"),
        }
    }

    async fn faulted_errors(c: &DIContainer) -> Vec<String> {
        c.image_repo
            .get_all()
            .await
            .unwrap()
            .into_iter()
            .filter(|image| image.status == ImageStatus::Faulted)
            .map(|image| image.error.unwrap())
            .collect()
    }

    fn image_dirs(guard: &TestDir) -> Vec<String> {
        let Ok(dirs) = std::fs::read_dir(guard.0.join("images")) else {
            return Vec::new();
        };
        let mut names: Vec<String> = dirs
            .map(|dir| dir.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn export_then_import_reproduces_the_image_under_a_new_id() {
        let (c, guard) = container().await;
        let source = ready_image(&c).await;
        let source = c
            .image_repo
            .set_root_partition(source.id, Some(2))
            .await
            .unwrap();

        let imported = c
            .archive_service
            .import(export(&c, source.id).await.as_slice())
            .await
            .unwrap();

        assert_ne!(imported.id, source.id);
        assert_eq!(imported.name, source.name);
        assert_eq!(imported.status, ImageStatus::Ready);
        assert_eq!(imported.captured_at, source.captured_at);
        assert_eq!(imported.root_partition, Some(2));
        let layout = |image: &Image| -> Vec<(i64, String, u64)> {
            image
                .partitions
                .iter()
                .map(|p| (p.partition_number, p.fstype.clone(), p.size_bytes))
                .collect()
        };
        assert_eq!(layout(&imported), layout(&source));
        assert_eq!(
            c.image_service
                .read_partition_table(imported.id)
                .await
                .unwrap(),
            TABLE
        );
        for number in [1, 2] {
            assert_eq!(
                std::fs::read(c.image_service.get_partition_path(imported.id, number)).unwrap(),
                std::fs::read(c.image_service.get_partition_path(source.id, number)).unwrap(),
            );
        }
        assert_eq!(
            image_dirs(&guard),
            [format!("img-{}", source.id), format!("img-{}", imported.id)]
        );
    }

    #[tokio::test]
    async fn export_writes_the_manifest_first_followed_by_the_checksummed_files() {
        let (c, _guard) = container().await;
        let source = ready_image(&c).await;

        let files = untar(&export(&c, source.id).await).await;

        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["manifest.json", "parttable.bin", "p-1.pcl", "p-2.pcl"]
        );
        let manifest: serde_json::Value = serde_json::from_slice(&files[0].1).unwrap();
        assert_eq!(
            manifest,
            json!({
                "format": 1,
                "name": "golden",
                "captured_at": source.captured_at.unwrap().to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
                "partition_table": { "file": "parttable.bin", "sha256": sha256(TABLE) },
                "partitions": [
                    { "number": 1, "fstype": "vfat", "size_bytes": 1u64 << 33, "file": "p-1.pcl", "sha256": sha256(&large()) },
                    { "number": 2, "fstype": "ntfs", "size_bytes": 1u64 << 33, "file": "p-2.pcl", "sha256": sha256(SMALL) },
                ],
            })
        );
        assert_eq!(files[1].1, TABLE);
        assert_eq!(files[2].1, large());
        assert_eq!(files[3].1, SMALL);
    }

    #[tokio::test]
    async fn a_root_partition_that_is_not_archived_is_rejected_without_an_image() {
        let (c, _guard) = container().await;
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&manifest(&[(1, SMALL)])).unwrap();
        manifest["root_partition"] = json!(2);
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let archive = tar(&[
            ("manifest.json", &manifest),
            ("parttable.bin", TABLE),
            ("p-1.pcl", SMALL),
        ])
        .await;

        let message = rejected_import(&c, &archive).await;

        assert!(message.contains("root partition 2"), "{message}");
        assert!(c.image_repo.get_all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn export_stream_ends_with_an_error_instead_of_a_clean_end_when_the_export_fails() {
        let (c, _guard) = container().await;
        let source = ready_image(&c).await;
        std::fs::remove_file(c.image_service.get_partition_path(source.id, 2)).unwrap();

        let chunks: Vec<io::Result<Bytes>> =
            c.archive_service.export_stream(source.id).collect().await;

        assert!(chunks.last().unwrap().is_err());
    }

    #[tokio::test]
    async fn exporting_an_image_that_is_not_ready_fails_without_writing() {
        let (c, _guard) = container().await;
        let image = c.image_repo.create_image("empty".into()).await.unwrap();

        let mut archive = Vec::new();
        let err = c
            .archive_service
            .export(image.id, &mut archive)
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::FailedPrecondition(_)), "{err:?}");
        assert!(archive.is_empty());
    }

    #[tokio::test]
    async fn a_tampered_byte_faults_the_import_and_leaves_no_files_behind() {
        let (c, guard) = container().await;
        let source = ready_image(&c).await;
        let mut archive = export(&c, source.id).await;
        let at = archive
            .windows(SMALL.len())
            .position(|window| window == SMALL)
            .unwrap();
        archive[at] ^= 0xff;

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "checksum mismatch for `p-2.pcl`");
        let errors = faulted_errors(&c).await;
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("checksum mismatch"), "{errors:?}");
        assert_eq!(image_dirs(&guard), [format!("img-{}", source.id)]);
    }

    #[tokio::test]
    async fn an_archive_that_does_not_start_with_the_manifest_is_rejected_without_an_image() {
        let (c, guard) = container().await;
        let archive = tar(&[("parttable.bin", TABLE), ("manifest.json", &manifest(&[]))]).await;

        let message = rejected_import(&c, &archive).await;

        assert_eq!(
            message,
            "archive must start with manifest.json, found `parttable.bin`"
        );
        assert!(c.image_repo.get_all().await.unwrap().is_empty());
        assert!(image_dirs(&guard).is_empty());
    }

    #[tokio::test]
    async fn an_unsupported_format_is_rejected_without_an_image() {
        let (c, _guard) = container().await;
        let archive = tar(&[("manifest.json", br#"{"format":2,"anything":"else"}"#)]).await;

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "unsupported archive format 2, expected 1");
        assert!(c.image_repo.get_all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_entry_not_listed_in_the_manifest_faults_the_import() {
        let (c, guard) = container().await;
        let archive = tar(&[
            ("manifest.json", &manifest(&[(1, SMALL)])),
            ("parttable.bin", TABLE),
            ("p-1.pcl", SMALL),
            ("notes.txt", b"hello"),
        ])
        .await;

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "unexpected archive entry `notes.txt`");
        assert_eq!(faulted_errors(&c).await.len(), 1);
        assert!(image_dirs(&guard).is_empty());
    }

    #[tokio::test]
    async fn a_duplicate_entry_faults_the_import() {
        let (c, guard) = container().await;
        let archive = tar(&[
            ("manifest.json", &manifest(&[(1, SMALL)])),
            ("parttable.bin", TABLE),
            ("p-1.pcl", SMALL),
            ("p-1.pcl", SMALL),
        ])
        .await;

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "duplicate archive entry `p-1.pcl`");
        assert_eq!(faulted_errors(&c).await.len(), 1);
        assert!(image_dirs(&guard).is_empty());
    }

    #[tokio::test]
    async fn a_file_listed_in_the_manifest_but_absent_faults_the_import() {
        let (c, guard) = container().await;
        let archive = tar(&[
            ("manifest.json", &manifest(&[(1, SMALL)])),
            ("parttable.bin", TABLE),
        ])
        .await;

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "archive is missing `p-1.pcl`");
        assert_eq!(faulted_errors(&c).await.len(), 1);
        assert!(image_dirs(&guard).is_empty());
    }

    #[tokio::test]
    async fn entries_that_escape_the_import_directory_are_rejected() {
        let (c, guard) = container().await;

        for path in ["../p-1.pcl", "/p-1.pcl", "nested/p-1.pcl"] {
            let archive = tar(&[
                ("manifest.json", &manifest(&[(1, SMALL)])),
                ("parttable.bin", TABLE),
                (path, SMALL),
            ])
            .await;

            let message = rejected_import(&c, &archive).await;

            assert_eq!(
                message,
                format!("archive entry `{path}` is not a plain file")
            );
        }
        assert_eq!(faulted_errors(&c).await.len(), 3);
        assert!(image_dirs(&guard).is_empty());
        assert!(!guard.0.join("p-1.pcl").exists());
    }

    #[tokio::test]
    async fn a_truncated_archive_faults_the_import() {
        let (c, guard) = container().await;
        let source = ready_image(&c).await;
        let archive = export(&c, source.id).await;

        let message = rejected_import(&c, &archive[..archive.len() / 2]).await;

        assert!(message.starts_with("malformed archive"), "{message}");
        assert_eq!(faulted_errors(&c).await.len(), 1);
        assert_eq!(image_dirs(&guard), [format!("img-{}", source.id)]);
    }

    #[tokio::test]
    async fn a_gnu_long_name_header_is_rejected_instead_of_renaming_the_next_entry() {
        let (c, guard) = container().await;
        let archive = typed_tar(&[
            ("././@LongLink", EntryType::GNULongName, b"manifest.json\0"),
            ("renamed", EntryType::Regular, &manifest(&[])),
            ("parttable.bin", EntryType::Regular, TABLE),
        ])
        .await;

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "archive entry `././@LongLink` is not a plain file");
        assert!(c.image_repo.get_all().await.unwrap().is_empty());
        assert!(image_dirs(&guard).is_empty());
    }

    #[tokio::test]
    async fn links_extension_headers_and_directories_are_rejected_as_partition_data() {
        let (c, guard) = container().await;

        for entry_type in [
            EntryType::Symlink,
            EntryType::Link,
            EntryType::Directory,
            EntryType::XHeader,
            EntryType::GNUSparse,
        ] {
            let archive = typed_tar(&[
                (
                    "manifest.json",
                    EntryType::Regular,
                    &manifest(&[(1, SMALL)]),
                ),
                ("parttable.bin", EntryType::Regular, TABLE),
                ("p-1.pcl", entry_type, SMALL),
            ])
            .await;

            let message = rejected_import(&c, &archive).await;

            assert_eq!(
                message, "archive entry `p-1.pcl` is not a plain file",
                "{entry_type:?}"
            );
        }
        assert_eq!(faulted_errors(&c).await.len(), 5);
        assert!(image_dirs(&guard).is_empty());
    }

    #[tokio::test]
    async fn a_partition_table_larger_than_one_mebibyte_is_rejected() {
        let (c, guard) = container().await;
        let largest = vec![7u8; 1024 * 1024];
        let too_large = vec![7u8; 1024 * 1024 + 1];

        let accepted = c
            .archive_service
            .import(
                tar(&[
                    ("manifest.json", &manifest_with_table(&largest, &[])),
                    ("parttable.bin", &largest),
                ])
                .await
                .as_slice(),
            )
            .await
            .unwrap();
        let message = rejected_import(
            &c,
            &tar(&[
                ("manifest.json", &manifest_with_table(&too_large, &[])),
                ("parttable.bin", &too_large),
            ])
            .await,
        )
        .await;

        assert_eq!(accepted.status, ImageStatus::Ready);
        assert_eq!(message, "parttable.bin exceeds 1048576 bytes");
        assert_eq!(image_dirs(&guard), [format!("img-{}", accepted.id)]);
    }

    #[tokio::test]
    async fn starting_the_service_faults_interrupted_imports_and_reclaims_their_files() {
        let (c, guard) = container().await;
        let ready = ready_image(&c).await;
        let interrupted = c.image_repo.start_import("half".into()).await.unwrap();
        let import_dir = c.image_service.import_dir(interrupted.id);
        std::fs::create_dir_all(&import_dir).unwrap();
        std::fs::write(format!("{import_dir}/p-1.pcl"), b"partial").unwrap();

        for _ in 0..2 {
            ArchiveService::start(c.image_repo.clone(), c.image_service.clone())
                .await
                .unwrap();
        }

        let faulted = c.image_repo.get(interrupted.id).await.unwrap();
        assert_eq!(faulted.status, ImageStatus::Faulted);
        assert_eq!(faulted.error.as_deref(), Some("import interrupted"));
        assert_eq!(
            c.image_repo.get_status(ready.id).await.unwrap(),
            ImageStatus::Ready
        );
        assert_eq!(image_dirs(&guard), [format!("img-{}", ready.id)]);
    }

    #[test]
    fn archive_file_names_only_contain_header_safe_characters() {
        assert_eq!(
            archive_file_name("Win 11 \"golden\"\r\n.v2"),
            "Win_11__golden___.v2.imaged"
        );
        assert_eq!(archive_file_name(""), "image.imaged");
    }
}
