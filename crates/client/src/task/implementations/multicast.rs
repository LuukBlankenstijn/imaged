use async_compression::tokio::bufread::ZstdDecoder;
use derive_more::Display;
use imaged_shared::get_multicast_port;
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::Command,
};
use tracing::{debug, info};

use super::ClientTaskExt;
use crate::{sys, task::PARTTABLE_TMP, transport::multicast::udp_receiver_stream};

#[derive(Clone, Display)]
#[display("multicast task")]
pub(crate) struct MulticastTask;

impl ClientTaskExt for MulticastTask {
    async fn handle_partition_table(
        &self,
        _: &crate::transport::ApiClient,
        device: &str,
    ) -> anyhow::Result<()> {
        let status = Command::new("sgdisk")
            .args(["--zap-all", device])
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("sgdisk --zap-all failed");
        }

        let port = get_multicast_port(0);
        let mut buffer: Vec<u8> = Vec::new();
        let mut data_stream = udp_receiver_stream(port).await?;
        if let Err(e) = data_stream.read_to_end(&mut buffer).await {
            anyhow::bail!("failed to read partition table stream to buffer: {e}");
        };
        tokio::fs::write(PARTTABLE_TMP, buffer).await?;
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
        _: &crate::transport::ApiClient,
        partition: crate::sys::disk::BlockDevice,
    ) -> anyhow::Result<()> {
        let Some(fstype) = &partition.fstype else {
            tracing::info!(name=%partition.name, "skipping partition with no fstype");
            return Ok(());
        };

        debug!(partition_number=%partition.find_partition_number()?, "starting partition download with udp-receiver");
        let port = get_multicast_port(partition.find_partition_number()?);
        let stream = udp_receiver_stream(port).await?;
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
            .map_err(|e| anyhow::anyhow!("failed piping udp-receiver into partclone: {e}"))?;

        // Close partclone's stdin so it observes EOF and can finish; otherwise
        // the still-open pipe and child.wait() deadlock each other.
        drop(child_stdin);

        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("partclone exited with error: {}", status);
        }

        Ok(())
    }

    async fn finalize(&self, _: &crate::transport::ApiClient) -> anyhow::Result<()> {
        tracing::info!(task=%self, "finished task successfully");
        sys::reboot()
    }
}
