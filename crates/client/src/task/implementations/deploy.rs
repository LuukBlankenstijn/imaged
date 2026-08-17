use std::io;

use async_compression::tokio::bufread::ZstdDecoder;
use derive_more::{Constructor, Display};
use futures::StreamExt;
use tokio::process::Command;
use tokio_util::io::StreamReader;
use tracing::{debug, info};

use super::ClientTaskExt;
use crate::{sys, task::PARTTABLE_TMP};

#[derive(Constructor, Clone, Display)]
#[display("deploy task {task_id}")]
pub(crate) struct DeployTask {
    task_id: i64,
}

impl ClientTaskExt for DeployTask {
    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        let status = Command::new("sgdisk")
            .args(["--zap-all", device])
            .kill_on_drop(true)
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("sgdisk --zap-all failed");
        }

        super::discard_disk(device).await;

        let data = api::deploy::download_partition_table(self.task_id).await?;
        tokio::fs::write(PARTTABLE_TMP, data).await?;
        let status = Command::new("sgdisk")
            .args([&format!("--load-backup={PARTTABLE_TMP}"), device])
            .kill_on_drop(true)
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("sgdisk --load-backup failed");
        }

        let status = Command::new("partprobe")
            .kill_on_drop(true)
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("partprobe failed");
        }

        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        Ok(())
    }

    async fn plan_partitions(
        &self,
        disk: &crate::sys::disk::BlockDevice,
    ) -> anyhow::Result<Vec<crate::sys::disk::PartitionTarget>> {
        super::image_partitions(self.task_id, disk).await
    }

    async fn handle_partition(
        &self,
        partition: crate::sys::disk::PartitionTarget,
    ) -> anyhow::Result<()> {
        debug!(partition_number=%partition.number, "starting partition download");
        let stream = api::deploy::download_partition_data(self.task_id, partition.number).await?;
        let mut decoder = ZstdDecoder::new(StreamReader::new(
            stream
                .into_inner()
                .map(|item| item.map_err(io::Error::other)),
        ));

        info!(partition_number=%partition.number, fstype=%partition.fstype, "restoring partition");
        let partclone_bin = partition.partclone_binary()?;
        let mut child = tokio::process::Command::new(partclone_bin)
            .args([
                "--restore",
                "--logfile",
                "/tmp/partclone-log",
                "--source",
                "-",
                "--output",
                &partition.device,
            ])
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let mut child_stdin = child.stdin.take().expect("stdin piped");

        tokio::io::copy(&mut decoder, &mut child_stdin)
            .await
            .map_err(|e| anyhow::anyhow!("failed piping partition data into partclone: {e}"))?;

        // Close partclone's stdin so it observes EOF and can finish; otherwise
        // the still-open pipe and child.wait() deadlock each other.
        drop(child_stdin);

        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("partclone exited with error: {}", status);
        }

        Ok(())
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        api::task::mark_finished(self.task_id).await?;
        tracing::info!(task=%self, "finished task successfully");
        // best effort disconnect
        let _ = api::event::disconnect().await;
        sys::reboot()
    }

    async fn finalize_error(&self, err: &str) -> anyhow::Result<()> {
        tracing::error!(task=%self, error=%err, "did not finish task successfully");
        api::task::mark_failed(self.task_id, err.to_string()).await?;
        Ok(())
    }
}
