use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, bail};
use async_compression::tokio::bufread::{ZstdDecoder, ZstdEncoder};
use tokio::io::{AsyncRead, BufReader};
use tokio::process::{Child, Command};

use crate::Filesystem;

const LOG_TAIL_LINES: usize = 20;

pub struct Partclone {
    child: Child,
    logfile: PathBuf,
}

impl Partclone {
    pub async fn wait(mut self) -> anyhow::Result<()> {
        let status = self.child.wait().await?;
        if !status.success() {
            bail!(
                "partclone exited with {status}{}",
                log_tail(&self.logfile).await
            );
        }
        Ok(())
    }
}

async fn log_tail(logfile: &Path) -> String {
    let Ok(bytes) = tokio::fs::read(logfile).await else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = &lines[lines.len().saturating_sub(LOG_TAIL_LINES)..];
    if tail.is_empty() {
        return String::new();
    }
    format!("\n{}", tail.join("\n"))
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
    Ok((
        Partclone {
            child,
            logfile: logfile.to_owned(),
        },
        ZstdEncoder::new(BufReader::new(stdout)),
    ))
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
    let copied = {
        let mut stdin = child.stdin.take().expect("stdin piped");
        tokio::io::copy(&mut decoder, &mut stdin).await
    };

    let status = child.wait().await?;
    match copied {
        Err(e) => {
            let piping = anyhow!("failed piping partition data into partclone: {e}");
            if status.success() {
                return Err(piping);
            }
            let tail = log_tail(logfile).await;
            Err(piping.context(format!("partclone exited with error: {status}{tail}")))
        }
        Ok(_) if !status.success() => {
            bail!(
                "partclone exited with error: {status}{}",
                log_tail(logfile).await
            )
        }
        Ok(_) => Ok(()),
    }
}
