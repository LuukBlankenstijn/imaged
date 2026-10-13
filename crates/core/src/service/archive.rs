use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use imaged_archive::manifest::Manifest;
use imaged_archive::{ArchiveReader, Contents, PartitionSource, Unpacked, write_archive};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::io::ReaderStream;

use crate::domain::image::{Image, ImageRepository, NewImagePartition};
use crate::domain::script::{NewScript, ScriptRepository};
use crate::service::image::ImageService;
use imaged_model::error::{AppError, Result};

const INTERRUPTED_IMPORT_ERROR: &str = "import interrupted";
const CHUNK_BYTES: usize = 256 * 1024;

pub struct ArchiveService {
    image_repo: Arc<dyn ImageRepository>,
    script_repo: Arc<dyn ScriptRepository>,
    image_service: Arc<ImageService>,
}

impl ArchiveService {
    pub async fn start(
        image_repo: Arc<dyn ImageRepository>,
        script_repo: Arc<dyn ScriptRepository>,
        image_service: Arc<ImageService>,
    ) -> Result<Self> {
        image_repo
            .fault_unfinished_imports(INTERRUPTED_IMPORT_ERROR)
            .await?;
        image_service.clear_import_dirs().await?;
        Ok(Self {
            image_repo,
            script_repo,
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
        let scripts = self.script_repo.get_for_image(image_id).await?;
        let contents = Contents {
            partition_table: PathBuf::from(self.image_service.get_partition_table_path(image_id)),
            partitions: image
                .partitions
                .into_iter()
                .map(|partition| PartitionSource {
                    path: PathBuf::from(
                        self.image_service
                            .get_partition_path(image_id, partition.partition_number),
                    ),
                    number: partition.partition_number,
                    fstype: partition.fstype,
                    size_bytes: partition.size_bytes,
                })
                .collect(),
            name: image.name,
            captured_at: image.captured_at,
            root_partition: image.root_partition,
            scripts: scripts
                .into_iter()
                .map(|script| imaged_archive::Script {
                    name: script.name,
                    body: script.body,
                    inputs: script.inputs,
                })
                .collect(),
        };
        write_archive(contents, writer).await?;
        Ok(())
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
        let reader = ArchiveReader::open(reader).await?;
        let image = self
            .image_repo
            .start_import(reader.manifest().name.clone())
            .await?;

        match self.receive(image.id, reader).await {
            Ok(imported) => Ok(imported),
            Err(error) => {
                self.discard(image.id, &error).await;
                Err(error)
            }
        }
    }

    async fn receive<R>(&self, image_id: i64, reader: ArchiveReader<'_, R>) -> Result<Image>
    where
        R: AsyncRead + Unpin + Send,
    {
        let import_dir = self.image_service.import_dir(image_id);
        let Unpacked { manifest, scripts } = reader.unpack(Path::new(&import_dir)).await?;
        tokio::fs::rename(&import_dir, self.image_service.image_dir(image_id))
            .await
            .map_err(store_failed)?;
        let scripts: Vec<NewScript> = scripts
            .into_iter()
            .map(|script| NewScript {
                name: script.name,
                body: script.body,
                inputs: script.inputs,
            })
            .collect();
        self.script_repo
            .import_for_image(image_id, &scripts)
            .await?;
        self.image_repo
            .finish_import(
                image_id,
                manifest.captured_at,
                &partitions(&manifest),
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

fn partitions(manifest: &Manifest) -> Vec<NewImagePartition> {
    manifest
        .partitions
        .iter()
        .map(|partition| NewImagePartition {
            partition_number: partition.number,
            fstype: partition.fstype.clone(),
            size_bytes: partition.size_bytes,
        })
        .collect()
}

fn store_failed(error: io::Error) -> AppError {
    AppError::Internal(format!("failed to store imported image: {error}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use imaged_model::script::{InputKind, ScriptInput};

    use super::*;
    use crate::di::DIContainer;
    use imaged_model::image::ImageStatus;

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

    async fn scripts_archive(guard: &TestDir, scripts: &[(&str, &str)]) -> Vec<u8> {
        let table = guard.0.join("table");
        std::fs::write(&table, TABLE).unwrap();
        let contents = Contents {
            name: "imported".into(),
            captured_at: None,
            partition_table: table,
            partitions: Vec::new(),
            root_partition: None,
            scripts: scripts
                .iter()
                .map(|&(name, body)| imaged_archive::Script {
                    name: name.into(),
                    body: body.into(),
                    inputs: Vec::new(),
                })
                .collect(),
        };
        let mut archive = Vec::new();
        write_archive(contents, &mut archive).await.unwrap();
        archive
    }

    fn replace(archive: &mut [u8], from: &[u8], to: &[u8]) {
        let at = archive
            .windows(from.len())
            .position(|window| window == from)
            .unwrap();
        archive[at..at + to.len()].copy_from_slice(to);
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

    fn hostname_input() -> ScriptInput {
        ScriptInput {
            name: "hostname".into(),
            label: "Hostname".into(),
            kind: InputKind::Text,
            required: true,
            default_value: None,
            per_machine: true,
            choices: Vec::new(),
        }
    }

    async fn scripted_image(c: &DIContainer) -> Image {
        let image = ready_image(c).await;
        let mut ids = Vec::new();
        for (name, body, inputs) in [
            (
                "set-hostname",
                "#!/bin/sh\nhostname \"$IMAGED_INPUT_HOSTNAME\"\n",
                vec![hostname_input()],
            ),
            ("cleanup", "rm -rf /var/cache/*\n", vec![]),
        ] {
            let script = NewScript {
                name: name.into(),
                body: body.into(),
                inputs,
            };
            ids.push(c.script_repo.create(&script).await.unwrap().id);
        }
        c.script_repo.set_for_image(image.id, &ids).await.unwrap();
        image
    }

    #[tokio::test]
    async fn scripts_travel_with_the_archive_and_replace_library_scripts_of_the_same_name() {
        let (c, _guard) = container().await;
        let source = scripted_image(&c).await;
        let archive = export(&c, source.id).await;

        let library = c.script_repo.get_all().await.unwrap();
        let hostname = library.iter().find(|s| s.name == "set-hostname").unwrap();
        let cleanup = library.iter().find(|s| s.name == "cleanup").unwrap();
        c.script_repo
            .update(
                hostname.id,
                &NewScript {
                    name: "set-hostname".into(),
                    body: "echo edited".into(),
                    inputs: vec![],
                },
            )
            .await
            .unwrap();
        c.script_repo
            .update(
                cleanup.id,
                &NewScript {
                    name: "tidy".into(),
                    body: cleanup.body.clone(),
                    inputs: vec![],
                },
            )
            .await
            .unwrap();

        let imported = c.archive_service.import(archive.as_slice()).await.unwrap();

        let scripts = c.script_repo.get_for_image(imported.id).await.unwrap();
        let listed: Vec<(&str, &str, usize)> = scripts
            .iter()
            .map(|s| (s.name.as_str(), s.body.as_str(), s.inputs.len()))
            .collect();
        assert_eq!(
            listed,
            [
                (
                    "set-hostname",
                    "#!/bin/sh\nhostname \"$IMAGED_INPUT_HOSTNAME\"\n",
                    1
                ),
                ("cleanup", "rm -rf /var/cache/*\n", 0),
            ]
        );
        assert_eq!(scripts[0].id, hostname.id);
        assert_ne!(scripts[1].id, cleanup.id);
        assert_eq!(c.script_repo.get(cleanup.id).await.unwrap().name, "tidy");
    }

    #[tokio::test]
    async fn a_tampered_script_faults_the_import_and_leaves_the_library_alone() {
        let (c, guard) = container().await;
        let mut archive = scripts_archive(&guard, &[("setup", "echo safe\n")]).await;
        replace(&mut archive, b"echo safe\n", b"echo evil\n");

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "checksum mismatch for `scripts/setup.sh`");
        assert!(c.script_repo.get_all().await.unwrap().is_empty());
        assert!(image_dirs(&guard).is_empty());
    }

    #[tokio::test]
    async fn an_invalid_script_after_a_valid_one_leaves_the_library_untouched() {
        let (c, guard) = container().await;
        c.script_repo
            .create(&NewScript {
                name: "setup".into(),
                body: "echo old".into(),
                inputs: vec![],
            })
            .await
            .unwrap();
        let archive = scripts_archive(&guard, &[("setup", "echo new"), ("blank", "\n")]).await;

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "script `blank` has an empty body");
        let library = c.script_repo.get_all().await.unwrap();
        let listed: Vec<(&str, &str)> = library
            .iter()
            .map(|s| (s.name.as_str(), s.body.as_str()))
            .collect();
        assert_eq!(listed, [("setup", "echo old")]);
        assert!(image_dirs(&guard).is_empty());
    }

    struct FailingAtEof<R>(R);

    impl<R: AsyncRead + Unpin> AsyncRead for FailingAtEof<R> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            let before = buf.filled().len();
            match std::pin::Pin::new(&mut self.0).poll_read(cx, buf) {
                std::task::Poll::Ready(Ok(())) if buf.filled().len() == before => {
                    std::task::Poll::Ready(Err(io::Error::other("download checksum mismatch")))
                }
                other => other,
            }
        }
    }

    #[tokio::test]
    async fn a_read_error_after_the_tar_terminator_faults_the_import() {
        let (c, guard) = container().await;
        let source = ready_image(&c).await;
        let mut archive = export(&c, source.id).await;
        archive.extend_from_slice(&[0; 1024]);
        archive.extend_from_slice(b"trailing bytes after the terminator");

        let message = match c
            .archive_service
            .import(FailingAtEof(archive.as_slice()))
            .await
        {
            Err(AppError::InvalidArgument(message)) => message,
            other => panic!("expected an invalid argument error, got {other:?}"),
        };

        assert_eq!(message, "malformed archive: download checksum mismatch");
        assert_eq!(
            faulted_errors(&c).await,
            [format!("invalid argument: {message}")]
        );
        assert_eq!(image_dirs(&guard), [format!("img-{}", source.id)]);

        let imported = c.archive_service.import(archive.as_slice()).await.unwrap();
        assert_eq!(imported.status, ImageStatus::Ready);
    }

    #[tokio::test]
    async fn an_unsupported_format_is_rejected_without_an_image() {
        let (c, guard) = container().await;
        let source = ready_image(&c).await;
        let mut archive = export(&c, source.id).await;
        replace(&mut archive, b"\"format\": 1", b"\"format\": 2");

        let message = rejected_import(&c, &archive).await;

        assert_eq!(message, "unsupported archive format 2, expected 1");
        let ids: Vec<i64> = c
            .image_repo
            .get_all()
            .await
            .unwrap()
            .iter()
            .map(|image| image.id)
            .collect();
        assert_eq!(ids, [source.id]);
        assert_eq!(image_dirs(&guard), [format!("img-{}", source.id)]);
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
    async fn starting_the_service_faults_interrupted_imports_and_reclaims_their_files() {
        let (c, guard) = container().await;
        let ready = ready_image(&c).await;
        let interrupted = c.image_repo.start_import("half".into()).await.unwrap();
        let import_dir = c.image_service.import_dir(interrupted.id);
        std::fs::create_dir_all(&import_dir).unwrap();
        std::fs::write(format!("{import_dir}/p-1.pcl"), b"partial").unwrap();

        for _ in 0..2 {
            ArchiveService::start(
                c.image_repo.clone(),
                c.script_repo.clone(),
                c.image_service.clone(),
            )
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
}
