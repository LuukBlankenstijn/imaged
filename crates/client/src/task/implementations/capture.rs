use async_compression::tokio::bufread::ZstdEncoder;
use derive_more::{Constructor, Display};
use tokio::{io::BufReader, process::Command};
use tokio_util::io::ReaderStream;

use super::ClientTaskExt;
use crate::task::PARTTABLE_TMP;

#[derive(Constructor, Clone, Display)]
#[display("capture task {task_id}")]
pub(crate) struct CaptureTask {
    task_id: i64,
}

impl ClientTaskExt for CaptureTask {
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

    async fn finalize(&self) -> anyhow::Result<()> {
        api::task::mark_finished(self.task_id).await?;
        tracing::info!(task=%self, "finished task successfully");
        Ok(())
    }

    async fn finalize_error(&self, err: &str) -> anyhow::Result<()> {
        tracing::error!(task=%self, error=%err, "did not finish task successfully");
        api::task::mark_failed(self.task_id, err.to_string()).await?;
        Ok(())
    }
}
