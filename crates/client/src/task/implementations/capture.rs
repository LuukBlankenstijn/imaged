use async_compression::tokio::bufread::ZstdEncoder;
use derive_more::{Constructor, Display};
use tokio::{io::BufReader, process::Command};
use tokio_util::io::ReaderStream;
use tracing::{info, warn};

use super::ClientTaskExt;
use crate::sys::disk::PartitionTarget;
use crate::sys::mount;
use crate::task::PARTTABLE_TMP;

#[derive(Constructor, Clone, Display)]
#[display("capture task {task_id}")]
pub struct CaptureTask {
    task_id: i64,
}

impl ClientTaskExt for CaptureTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        let status = Command::new("sgdisk")
            .args(["--backup", PARTTABLE_TMP, device])
            .kill_on_drop(true)
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("sgdisk failed");
        }

        let bytes = tokio::fs::read(PARTTABLE_TMP).await?;
        api::capture::partition_table(self.task_id, bytes.into()).await?;

        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        Ok(())
    }

    async fn handle_partition(
        &self,
        partition: crate::sys::disk::PartitionTarget,
    ) -> anyhow::Result<()> {
        let partclone_bin = partition.partclone_binary()?;
        let mut child = tokio::process::Command::new(partclone_bin)
            .args([
                "--clone",
                "--logfile",
                "/tmp/partclone-log",
                "--source",
                &partition.device,
                "--output",
                "-",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout piped");
        let compressed = ZstdEncoder::new(BufReader::new(stdout));

        api::capture::upload_partition_data(
            self.task_id,
            partition.number,
            partition.fstype,
            partition.size as i64,
            ReaderStream::new(compressed).into(),
        )
        .await?;

        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("partclone exited with {}", status);
        }
        Ok(())
    }

    async fn after_partitions(&self, partitions: &[PartitionTarget]) -> anyhow::Result<()> {
        let mut systems = Vec::new();
        for partition in partitions.iter().filter(|p| p.is_ext()) {
            match mount::has_os_release(&partition.device, &partition.fstype) {
                Ok(true) => systems.push(partition.number),
                Ok(false) => {}
                Err(e) => {
                    warn!(partition=%partition.number, error=%format!("{e:#}"), "could not look for an operating system")
                }
            }
        }
        match systems.as_slice() {
            [root] => match api::capture::root_partition(self.task_id, *root).await {
                Ok(()) => info!(partition=%root, "reported the root partition"),
                Err(e) => warn!(partition=%root, error=%e, "could not report the root partition"),
            },
            _ => info!(?systems, "no single partition holds /etc/os-release"),
        }
        Ok(())
    }
}
