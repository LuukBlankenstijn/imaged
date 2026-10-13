use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::process::Command;

use crate::Filesystem;

pub async fn has_os_release(source: &Path) -> anyhow::Result<bool> {
    let output = Command::new("debugfs")
        .args(["-R", "stat /etc/os-release"])
        .arg(source)
        .kill_on_drop(true)
        .output()
        .await
        .context("running debugfs")?;
    if output.stdout.starts_with(b"Inode:") {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("File not found by ext2_lookup") {
        return Ok(false);
    }
    bail!("{}", stderr.trim())
}

pub async fn find_root_partition(partitions: &[(i64, Filesystem, &Path)]) -> Option<i64> {
    let mut systems = Vec::new();
    for &(number, filesystem, source) in partitions {
        if !filesystem.is_ext() {
            continue;
        }
        match has_os_release(source).await {
            Ok(true) => systems.push(number),
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(partition=%number, error=%format!("{e:#}"), "could not look for an operating system")
            }
        }
    }
    match systems.as_slice() {
        [root] => Some(*root),
        _ => {
            tracing::info!(?systems, "no single partition holds /etc/os-release");
            None
        }
    }
}
