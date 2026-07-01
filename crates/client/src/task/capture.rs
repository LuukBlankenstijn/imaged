use derive_more::Constructor;
use tokio::{io::BufReader, process::Command};
use async_compression::tokio::bufread::ZstdEncoder;

use super::{PARTTABLE_TMP, types::ClientTask};

#[derive(Constructor, Clone)]
pub struct CaptureTask {
    task_id: i64,
}

impl ClientTask for CaptureTask {
    async fn handle_partition_table(
        &self,
        api: &crate::transport::ApiClient,
        device: &str,
    ) -> anyhow::Result<()> {
        let status = Command::new("sgdisk")
            .args(["--backup", PARTTABLE_TMP, device])
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("sgdisk failed");
        }

        let bytes = tokio::fs::read(PARTTABLE_TMP).await?;
        api.upload_parttable(self.task_id, bytes).await?;

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
        let partclone_bin = partition
            .get_partclone_binary()
            .ok_or_else(|| anyhow::anyhow!("filetype not supported: {fstype}"))?;
        let mut child = tokio::process::Command::new(partclone_bin)
            .args([
                "--clone",
                "--logfile",
                "/tmp/partclone-log",
                "--source",
                &partition.get_device(),
                "--output",
                "-",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout piped");
        let compressed = ZstdEncoder::new(BufReader::new(stdout));

        api.upload_partition_data(
            self.task_id,
            partition.find_partition_number()?,
            fstype,
            partition.size,
            compressed,
        )
        .await?;

        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("partclone exited with {}", status);
        }
        Ok(())
    }
}
