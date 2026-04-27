use std::sync::Arc;

use anyhow::Result;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{ClientState, sys::disk::BlockDevice, transport::ApiClient};

const PARTTABLE_TMP: &str = "/parttable.bin";

pub async fn run(state: Arc<ClientState>, image_id: i64, cancel: CancellationToken) -> Result<()> {
    let disk = crate::sys::disk::find_target_disk().await?;
    let device = format!("/dev/{}", disk.name);

    capture_partition_table(&state, image_id, &device).await?;
    if cancel.is_cancelled() {
        anyhow::bail!("cancelled");
    }

    for partition in disk.children.into_iter() {
        capture_partition(&state.http, image_id, partition).await?;
        if cancel.is_cancelled() {
            anyhow::bail!("cancelled");
        }
    }

    state.http.mark_capture_finished(image_id).await?;

    // 3.mark capture as done at the server
    Ok(())
}

async fn capture_partition_table(
    state: &ClientState,
    image_id: i64,
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
    state.http.upload_parttable(image_id, bytes).await?;

    let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
    Ok(())
}

async fn capture_partition(
    api: &ApiClient,
    image_id: i64,
    partition: BlockDevice,
) -> anyhow::Result<()> {
    let Some(fstype) = &partition.fstype else {
        tracing::info!(name=%partition.name, "skipping partition with no fstype");
        return Ok(());
    };
    let bin = partition
        .get_partclone_binary()
        .ok_or_else(|| anyhow::anyhow!("filetype not supported: {fstype}"))?;
    let mut child = tokio::process::Command::new(bin)
        .args([
            "-c",
            "-s",
            &partition.get_device(),
            "-L",
            "/tmp/partclone-log",
            "-o",
            "-",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()?;

    let stdout = child.stdout.take().expect("stdout piped");

    api.upload_partition_data(
        image_id,
        partition.find_partition_number()?,
        fstype,
        partition.size,
        stdout,
    )
    .await?;

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("partclone exited with {}", status);
    }
    Ok(())
}
