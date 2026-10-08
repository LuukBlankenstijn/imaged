use std::{
    future::Future,
    io::{self, IoSlice},
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

use axum::serve::Listener;
use socket2::SockRef;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

use crate::DEAD_CONNECTION_TIMEOUT;

pub struct AbortableListener {
    listener: TcpListener,
    abort: CancellationToken,
}

pub async fn bind(address: SocketAddr, abort: CancellationToken) -> io::Result<AbortableListener> {
    Ok(AbortableListener {
        listener: TcpListener::bind(address).await?,
        abort,
    })
}

impl Listener for AbortableListener {
    type Io = AbortableStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, address) = Listener::accept(&mut self.listener).await;
        if let Err(e) = detect_dead_connections(SockRef::from(&stream)) {
            tracing::warn!(error = %e, "failed to set connection death timeout");
        }
        (AbortableStream::new(stream, &self.abort), address)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn detect_dead_connections(socket: SockRef<'_>) -> io::Result<()> {
    socket.set_tcp_user_timeout(Some(DEAD_CONNECTION_TIMEOUT))
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn detect_dead_connections(socket: SockRef<'_>) -> io::Result<()> {
    const PROBES: u32 = 3;
    let probe_interval = DEAD_CONNECTION_TIMEOUT / (PROBES + 1);
    socket.set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(probe_interval)
            .with_interval(probe_interval)
            .with_retries(PROBES),
    )
}

pub struct AbortableStream {
    stream: TcpStream,
    read_aborted: Pin<Box<WaitForCancellationFutureOwned>>,
    write_aborted: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl AbortableStream {
    fn new(stream: TcpStream, abort: &CancellationToken) -> Self {
        Self {
            stream,
            read_aborted: Box::pin(abort.clone().cancelled_owned()),
            write_aborted: Box::pin(abort.clone().cancelled_owned()),
        }
    }
}

fn aborted(abort: &mut Pin<Box<WaitForCancellationFutureOwned>>, cx: &mut Context<'_>) -> bool {
    abort.as_mut().poll(cx).is_ready()
}

fn connection_aborted<T>() -> Poll<io::Result<T>> {
    Poll::Ready(Err(io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "imaged stopped serving",
    )))
}

impl AsyncRead for AbortableStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if aborted(&mut self.read_aborted, cx) {
            return connection_aborted();
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for AbortableStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if aborted(&mut self.write_aborted, cx) {
            return connection_aborted();
        }
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if aborted(&mut self.write_aborted, cx) {
            return connection_aborted();
        }
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
