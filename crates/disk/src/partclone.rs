mod status;

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll, ready};

use anyhow::{anyhow, bail};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::process::{Child, ChildStdout, Command};
use tokio::task::JoinHandle;

use crate::Progress;
use crate::imaging::Captured;

const LOG_TAIL_LINES: usize = 20;

pub fn capture(
    binary: &str,
    source: &Path,
    logfile: &Path,
    on_progress: impl Fn(Progress) + Send + 'static,
) -> anyhow::Result<Captured> {
    let mut child = partclone(binary, "--clone", logfile)
        .arg("--source")
        .arg(source)
        .args(["--output", "-"])
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let progress = tokio::spawn(status::follow(stderr, on_progress));
    Ok(Box::new(Output {
        stdout,
        exit: Some(Box::pin(exited(child, progress, logfile.to_owned()))),
    }))
}

pub async fn restore(
    binary: &str,
    mut data: impl AsyncRead + Unpin,
    target: &Path,
    logfile: &Path,
    on_progress: impl Fn(Progress),
) -> anyhow::Result<()> {
    let mut child = partclone(binary, "--restore", logfile)
        .args(["--source", "-", "--output"])
        .arg(target)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("stdin piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let (copied, ()) = tokio::join!(
        async move { tokio::io::copy(&mut data, &mut stdin).await },
        status::follow(stderr, on_progress),
    );

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

fn partclone(binary: &str, mode: &str, logfile: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg(mode)
        .args(status::FLAGS)
        .arg("--logfile")
        .arg(logfile)
        .env("LC_ALL", "C")
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

struct Output {
    stdout: ChildStdout,
    exit: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send>>>,
}

impl AsyncRead for Output {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let output = &mut *self;
        let Some(exit) = &mut output.exit else {
            return Poll::Ready(Ok(()));
        };
        let filled = buf.filled().len();
        ready!(Pin::new(&mut output.stdout).poll_read(cx, buf))?;
        if buf.filled().len() > filled || buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let result = ready!(exit.as_mut().poll(cx));
        output.exit = None;
        Poll::Ready(result)
    }
}

async fn exited(mut child: Child, progress: JoinHandle<()>, logfile: PathBuf) -> io::Result<()> {
    let status = child.wait().await?;
    let _ = progress.await;
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
