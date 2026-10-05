use async_compression::tokio::bufread::ZstdDecoder;
use derive_more::{Constructor, Display};
use tokio::{io::AsyncReadExt, process::Command};
use tracing::{debug, info};

use super::ClientTaskExt;
use crate::{task::PARTTABLE_TMP, transport::multicast::multicast_stream};

#[derive(Clone, Display, Constructor)]
#[display("multicast task")]
pub struct MulticastTask {
    task_id: i64,
}

impl ClientTaskExt for MulticastTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        let mut buffer: Vec<u8> = Vec::new();
        multicast_stream(self.task_id, 0)?
            .read_to_end(&mut buffer)
            .await
            .map_err(|e| anyhow::anyhow!("failed to receive the partition table: {e}"))?;
        tokio::fs::write(PARTTABLE_TMP, buffer).await?;

        let status = Command::new("sgdisk")
            .args(["--zap-all", device])
            .kill_on_drop(true)
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("sgdisk --zap-all failed");
        }

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
        debug!(partition_number=%partition.number, "starting partition download over multicast");
        let stream = multicast_stream(self.task_id, partition.number)?;
        let mut decoder = ZstdDecoder::new(stream);

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
            .map_err(|e| anyhow::anyhow!("failed piping multicast stream into partclone: {e}"))?;

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
        super::finish_and_reboot(self).await
    }
}
