use std::path::Path;
use std::process::Output;

use anyhow::{Context as _, bail};
use tokio::process::Command;
use tracing::info;

const E2FSCK_CORRECTED: i32 = 0b11;

pub async fn shrink_ext(source: &Path) -> anyhow::Result<u64> {
    let checked = run("e2fsck", &["-f", "-p"], source).await?;
    if checked
        .status
        .code()
        .is_none_or(|code| code & !E2FSCK_CORRECTED != 0)
    {
        failed("e2fsck -f -p", source, &checked)?;
    }
    let resized = run("resize2fs", &["-M"], source).await?;
    if !resized.status.success() {
        failed("resize2fs -M", source, &resized)?;
    }
    let size_bytes = fs_size(source).await?;
    info!(size_bytes, "shrank the ext filesystem");
    Ok(size_bytes)
}

async fn fs_size(source: &Path) -> anyhow::Result<u64> {
    let dumped = Command::new("dumpe2fs")
        .arg("-h")
        .arg(source)
        .kill_on_drop(true)
        .output()
        .await
        .context("running dumpe2fs")?;
    if !dumped.status.success() {
        failed("dumpe2fs -h", source, &dumped)?;
    }
    let header = String::from_utf8_lossy(&dumped.stdout);
    let field = |name: &str| -> anyhow::Result<u64> {
        header
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .with_context(|| format!("dumpe2fs did not report the {name}"))?
            .trim()
            .parse()
            .with_context(|| format!("parsing the {name} reported by dumpe2fs"))
    };
    field("Block count")?
        .checked_mul(field("Block size")?)
        .context("the filesystem size overflows")
}

async fn run(program: &str, args: &[&str], source: &Path) -> anyhow::Result<Output> {
    let output = Command::new(program)
        .args(args)
        .arg(source)
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("running {program}"))?;
    for line in printed(&output) {
        info!("{program}: {line}");
    }
    Ok(output)
}

fn failed(command: &str, source: &Path, output: &Output) -> anyhow::Result<()> {
    bail!(
        "{command} {} failed with {}: {}",
        source.display(),
        output.status,
        printed(output).join(" ")
    )
}

fn printed(output: &Output) -> Vec<String> {
    [&output.stdout, &output.stderr]
        .into_iter()
        .flat_map(|bytes| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}
