use std::path::Path;
use std::process::Stdio;

use anyhow::{anyhow, bail};
use async_compression::tokio::bufread::{ZstdDecoder, ZstdEncoder};
use tokio::io::{AsyncRead, BufReader};
use tokio::process::{Child, Command};

use crate::Filesystem;

pub struct Partclone(Child);

impl Partclone {
    pub async fn wait(mut self) -> anyhow::Result<()> {
        let status = self.0.wait().await?;
        if !status.success() {
            bail!("partclone exited with {status}");
        }
        Ok(())
    }
}

pub fn clone_compressed(
    filesystem: Filesystem,
    source: &Path,
    logfile: &Path,
) -> anyhow::Result<(Partclone, impl AsyncRead + Send + Unpin + 'static)> {
    let mut child = Command::new(filesystem.partclone_binary())
        .arg("--clone")
        .arg("--logfile")
        .arg(logfile)
        .arg("--source")
        .arg(source)
        .args(["--output", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout piped");
    Ok((Partclone(child), ZstdEncoder::new(BufReader::new(stdout))))
}

pub async fn restore_compressed(
    filesystem: Filesystem,
    compressed: impl AsyncRead + Unpin,
    target: &Path,
    logfile: &Path,
) -> anyhow::Result<()> {
    let mut decoder = ZstdDecoder::new(BufReader::new(compressed));
    let mut child = Command::new(filesystem.partclone_binary())
        .arg("--restore")
        .arg("--logfile")
        .arg(logfile)
        .args(["--source", "-", "--output"])
        .arg(target)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    {
        let mut stdin = child.stdin.take().expect("stdin piped");
        tokio::io::copy(&mut decoder, &mut stdin)
            .await
            .map_err(|e| anyhow!("failed piping partition data into partclone: {e}"))?;
    }

    let status = child.wait().await?;
    if !status.success() {
        bail!("partclone exited with error: {status}");
    }
    Ok(())
}
