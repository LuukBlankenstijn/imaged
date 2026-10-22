use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::process::Command;

pub async fn fstype(source: &Path) -> anyhow::Result<Option<String>> {
    let output = Command::new("blkid")
        .args(["-p", "-o", "value", "-s", "TYPE"])
        .arg(source)
        .kill_on_drop(true)
        .output()
        .await
        .context("running blkid")?;
    match output.status.code() {
        Some(0) => {
            let fstype = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            Ok((!fstype.is_empty()).then_some(fstype))
        }
        Some(2) => Ok(None),
        _ => bail!(
            "blkid could not probe {}: {}",
            source.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}
