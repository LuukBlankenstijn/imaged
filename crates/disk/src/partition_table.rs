use std::ffi::{OsStr, OsString};
use std::path::Path;

use anyhow::bail;
use tokio::process::Command;
use tracing::info;

pub async fn backup_partition_table(disk: &Path, backup: &Path) -> anyhow::Result<()> {
    run(
        "sgdisk",
        &[option("--backup=", backup).as_os_str(), disk.as_os_str()],
    )
    .await
}

pub async fn restore_partition_table(disk: &Path, backup: &Path) -> anyhow::Result<()> {
    run("sgdisk", &[OsStr::new("--zap-all"), disk.as_os_str()]).await?;
    run(
        "sgdisk",
        &[
            option("--load-backup=", backup).as_os_str(),
            disk.as_os_str(),
        ],
    )
    .await?;
    run("partprobe", &[]).await
}

fn option(name: &str, value: &Path) -> OsString {
    let mut option = OsString::from(name);
    option.push(value);
    option
}

async fn run(program: &str, args: &[&OsStr]) -> anyhow::Result<()> {
    let output = Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await?;
    let printed =
        [output.stdout, output.stderr].map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let lines: Vec<&str> = printed
        .iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    for line in &lines {
        info!("{program}: {line}");
    }
    if !output.status.success() {
        let first = args
            .first()
            .map_or_else(String::new, |arg| format!(" {}", arg.display()));
        bail!(
            "{program}{first} failed with {}: {}",
            output.status,
            lines.join(" ")
        );
    }
    Ok(())
}
