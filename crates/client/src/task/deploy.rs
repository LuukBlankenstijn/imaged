use derive_more::Constructor;
use tokio::{io::BufReader, process::Command};
use tracing::{debug, info};
use async_compression::tokio::bufread::ZstdDecoder;

use super::{PARTTABLE_TMP, types::ClientTask};

#[derive(Constructor, Clone)]
pub struct DeployTask {
    task_id: i64,
}

impl ClientTask for DeployTask {
    async fn handle_partition_table(
        &self,
        api: &crate::transport::ApiClient,
        device: &str,
    ) -> anyhow::Result<()> {
        let status = Command::new("sgdisk")
            .args(["--zap-all", device])
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("sgdisk --zap-all failed");
        }

        let data = api.download_parttable(self.task_id).await?;
        tokio::fs::write(PARTTABLE_TMP, data).await?;
        let status = Command::new("sgdisk")
            .args([&format!("--load-backup={PARTTABLE_TMP}"), device])
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("sgdisk --load-backup failed");
        }

        let status = Command::new("partprobe").status().await?;
        if !status.success() {
            anyhow::bail!("partprobe failed");
        }

        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        Ok(())
    }

    async fn handle_partition(
        &self,
        api: &crate::transport::ApiClient,
        partition: crate::sys::disk::BlockDevice,
    ) -> anyhow::Result<()> {
        let Some(fstype) = &partition.fstype else {
            tracing::info!(name=%partition.name, "skipping partition with no fstype");
            return Ok(());
        };

        debug!(partition_number=%partition.find_partition_number()?, "starting partition download");
        let stream = api
            .download_partition_data(self.task_id, partition.find_partition_number()?)
            .await?;
        let mut decoder = ZstdDecoder::new(BufReader::new(stream));

        info!(partition_number=%partition.find_partition_number()?, "restoring partition");
        let partclone_bin = partition
            .get_partclone_binary()
            .ok_or_else(|| anyhow::anyhow!("filetype not supported: {fstype}"))?;
        let mut child = tokio::process::Command::new(partclone_bin)
            .args([
                "--restore",
                "--logfile",
                "/tmp/partclone-log",
                "--source",
                "-",
                "--output",
                &partition.get_device(),
            ])
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .stdin(std::process::Stdio::piped())
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
}
