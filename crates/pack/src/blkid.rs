use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::process::Command;

#[derive(Debug, Default)]
pub struct Probe {
    pub fstype: Option<String>,
    pub uuid: Option<String>,
    pub label: Option<String>,
    pub pttype: Option<String>,
}

pub async fn probe(source: &Path) -> anyhow::Result<Probe> {
    let output = Command::new("blkid")
        .args(["-p", "-d", "-o", "export"])
        .arg(source)
        .kill_on_drop(true)
        .output()
        .await
        .context("running blkid")?;
    match output.status.code() {
        Some(0) => Ok(parse(&String::from_utf8_lossy(&output.stdout))),
        Some(2) => Ok(Probe::default()),
        _ => bail!(
            "blkid could not probe {}: {}",
            source.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

fn parse(export: &str) -> Probe {
    let mut probe = Probe::default();
    for (key, value) in export.lines().filter_map(|line| line.split_once('=')) {
        let slot = match key {
            "TYPE" => &mut probe.fstype,
            "UUID" => &mut probe.uuid,
            "LABEL" => &mut probe.label,
            "PTTYPE" => &mut probe.pttype,
            _ => continue,
        };
        if !value.is_empty() {
            *slot = Some(value.to_owned());
        }
    }
    probe
}
