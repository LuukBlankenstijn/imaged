use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll, ready};

use anyhow::{anyhow, bail};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::process::{Child, ChildStdout, Command};

use crate::imaging::Captured;

const LOG_TAIL_LINES: usize = 20;

pub fn capture(binary: &str, source: &Path, logfile: &Path) -> anyhow::Result<Captured> {
    let mut child = Command::new(binary)
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
    Ok(Box::new(Output::Reading {
        stdout,
        child,
        logfile: logfile.to_owned(),
    }))
}

pub async fn restore(
    binary: &str,
    mut data: impl AsyncRead + Unpin,
    target: &Path,
    logfile: &Path,
) -> anyhow::Result<()> {
    let mut child = Command::new(binary)
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
        tokio::io::copy(&mut data, &mut stdin).await
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

enum Output {
    Reading {
        stdout: ChildStdout,
        child: Child,
        logfile: PathBuf,
    },
    Exiting(Pin<Box<dyn Future<Output = io::Result<()>> + Send>>),
    Done,
}

impl AsyncRead for Output {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            match &mut *self {
                Output::Reading { stdout, .. } => {
                    let filled = buf.filled().len();
                    ready!(Pin::new(stdout).poll_read(cx, buf))?;
                    if buf.filled().len() > filled || buf.remaining() == 0 {
                        return Poll::Ready(Ok(()));
                    }
                    let Output::Reading { child, logfile, .. } =
                        std::mem::replace(&mut *self, Output::Done)
                    else {
                        unreachable!("matched Reading above");
                    };
                    *self = Output::Exiting(Box::pin(exited(child, logfile)));
                }
                Output::Exiting(exit) => {
                    let result = ready!(exit.as_mut().poll(cx));
                    *self = Output::Done;
                    return Poll::Ready(result);
                }
                Output::Done => return Poll::Ready(Ok(())),
            }
        }
    }
}

async fn exited(mut child: Child, logfile: PathBuf) -> io::Result<()> {
    let status = child.wait().await?;
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "partclone exited with {status}{}",
        log_tail(&logfile).await
    )))
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
