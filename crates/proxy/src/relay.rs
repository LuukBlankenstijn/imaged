use std::future;
use std::io;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
    ReadBuf,
};
use tokio::time::{Instant, sleep_until};

use crate::request::Body;

const MAX_CHUNK_LINE_LEN: u64 = 4096;

pub(crate) struct Traffic {
    up: AtomicU64,
    down: AtomicU64,
    last_activity: Mutex<Instant>,
}

#[derive(Clone, Copy)]
pub(crate) enum Direction {
    Up,
    Down,
}

impl Traffic {
    pub(crate) fn new() -> Self {
        Self {
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            last_activity: Mutex::new(Instant::now()),
        }
    }

    pub(crate) fn up(&self) -> u64 {
        self.up.load(Ordering::Relaxed)
    }

    pub(crate) fn down(&self) -> u64 {
        self.down.load(Ordering::Relaxed)
    }

    fn record(&self, direction: Direction, bytes: usize) {
        let counter = match direction {
            Direction::Up => &self.up,
            Direction::Down => &self.down,
        };
        counter.fetch_add(bytes as u64, Ordering::Relaxed);
        *self.lock_last_activity() = Instant::now();
    }

    fn lock_last_activity(&self) -> std::sync::MutexGuard<'_, Instant> {
        self.last_activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) async fn idle(&self, timeout: Option<Duration>) {
        let Some(timeout) = timeout else {
            return future::pending().await;
        };
        loop {
            let deadline = *self.lock_last_activity() + timeout;
            if Instant::now() >= deadline {
                return;
            }
            sleep_until(deadline).await;
        }
    }
}

pub(crate) struct Metered<'a, S> {
    inner: S,
    traffic: &'a Traffic,
    direction: Direction,
}

impl<'a, S> Metered<'a, S> {
    pub(crate) fn new(inner: S, traffic: &'a Traffic, direction: Direction) -> Self {
        Self {
            inner,
            traffic,
            direction,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Metered<'_, S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        let read = buf.filled().len() - before;
        if read > 0 {
            this.traffic.record(this.direction, read);
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Metered<'_, S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

pub(crate) async fn tunnel<C, U>(client: &mut C, upstream: &mut U) -> io::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    tokio::io::copy_bidirectional(client, upstream).await?;
    Ok(())
}

pub(crate) async fn forward<C, U>(
    client: &mut C,
    upstream: &mut U,
    early_data: &[u8],
    body: Body,
) -> io::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let (client_rx, mut client_tx) = tokio::io::split(client);
    let (mut upstream_rx, mut upstream_tx) = tokio::io::split(upstream);
    let upload = async {
        let mut request_body = BufReader::new(early_data.chain(client_rx));
        match body {
            Body::Length(length) => copy_exact(&mut request_body, &mut upstream_tx, length).await?,
            Body::Chunked => copy_chunked(&mut request_body, &mut upstream_tx).await?,
        }
        upstream_tx.flush().await
    };
    let download = async {
        tokio::io::copy(&mut upstream_rx, &mut client_tx).await?;
        client_tx.flush().await
    };
    tokio::pin!(upload, download);
    tokio::select! {
        result = &mut download => return result,
        result = &mut upload => result?,
    }
    download.await
}

async fn copy_exact<R, W>(reader: &mut R, writer: &mut W, length: u64) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let copied = tokio::io::copy(&mut (&mut *reader).take(length), writer).await?;
    if copied == length {
        Ok(())
    } else {
        Err(io::ErrorKind::UnexpectedEof.into())
    }
}

async fn copy_chunked<R, W>(reader: &mut R, writer: &mut W) -> io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let size_line = copy_line(reader, writer).await?;
        let size = match httparse::parse_chunk_size(&size_line) {
            Ok(httparse::Status::Complete((_, size))) => size,
            _ => return Err(invalid_chunk()),
        };
        if size == 0 {
            break;
        }
        copy_exact(reader, writer, size).await?;
        if copy_line(reader, writer).await? != b"\r\n" {
            return Err(invalid_chunk());
        }
    }
    while copy_line(reader, writer).await? != b"\r\n" {}
    Ok(())
}

async fn copy_line<R, W>(reader: &mut R, writer: &mut W) -> io::Result<Vec<u8>>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = Vec::new();
    (&mut *reader)
        .take(MAX_CHUNK_LINE_LEN)
        .read_until(b'\n', &mut line)
        .await?;
    if !line.ends_with(b"\n") {
        return Err(invalid_chunk());
    }
    writer.write_all(&line).await?;
    Ok(line)
}

fn invalid_chunk() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid chunked request body")
}
