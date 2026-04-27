use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{sys::disk::BlockDevice, task::ClientState, transport::ApiClient};

const PARTTABLE_TMP: &str = "/parttable.bin";

pub async fn run(state: Arc<ClientState>, image_id: i64, cancel: CancellationToken) -> Result<()> {
    let disk = crate::sys::disk::find_target_disk().await?;
    let device = format!("/dev/{}", disk.name);

    info!("trying to restore partition table");
    restore_partition_table(&state.http, image_id, &device).await?;
    if cancel.is_cancelled() {
        anyhow::bail!("cancelled");
    }

    // update all partitions
    let disk = crate::sys::disk::find_target_disk().await?;

    for partition in disk.children.into_iter() {
        restore_partition(&state.http, partition, image_id).await?;
        if cancel.is_cancelled() {
            anyhow::bail!("cancelled");
        }
    }

    state.http.mark_deploy_finished(image_id).await?;
    Ok(())
}

async fn restore_partition(
    api: &ApiClient,
    partition: BlockDevice,
    image_id: i64,
) -> anyhow::Result<()> {
    let Some(fstype) = &partition.fstype else {
        tracing::info!(name=%partition.name, "skipping partition with no fstype");
        return Ok(());
    };

    debug!(partition_number=%partition.find_partition_number()?, "starting partition download");
    let mut stream = api
        .download_partition_data(image_id, partition.find_partition_number()?)
        .await?;

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

    let copy_task = tokio::io::copy(&mut stream, &mut child_stdin);

    let (copy_result, status_result) = tokio::join!(copy_task, child.wait());

    copy_result?;
    let status = status_result?;
    if !status.success() {
        anyhow::bail!("partclone exited with error: {}", status);
    }

    Ok(())
}

async fn restore_partition_table(
    api: &ApiClient,
    image_id: i64,
    device: &str,
) -> anyhow::Result<()> {
    let status = Command::new("sgdisk")
        .args(["--zap-all", device])
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("sgdisk --zap-all failed");
    }

    let data = api.download_parttable(image_id).await?;
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
