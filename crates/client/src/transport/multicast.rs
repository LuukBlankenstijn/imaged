use std::net::{IpAddr, Ipv4Addr};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use imaged_shared::MULTICAST_GROUP_ADDRESS;
use scuttlecast::receiver::Receiver;
use scuttlecast::state::ReceiveState;
use tokio::io::{AsyncRead, DuplexStream, ReadBuf};
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;
use tracing::{info, warn};

use crate::sys;

const JOIN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const PIPE_CAPACITY: usize = 1024 * 1024;
const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(2);

struct MulticastStream {
    pipe: DuplexStream,
    _session: AbortOnDropHandle<()>,
}

impl AsyncRead for MulticastStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().pipe).poll_read(cx, buf)
    }
}

fn local_ipv4() -> anyhow::Result<Ipv4Addr> {
    match sys::get_ip() {
        Some(IpAddr::V4(v4)) => Ok(v4),
        other => anyhow::bail!("no local IPv4 address to join the multicast group on: {other:?}"),
    }
}

async fn log_progress(port: u16, progress: watch::Receiver<ReceiveState>) {
    let mut ticker = tokio::time::interval(PROGRESS_LOG_INTERVAL);
    loop {
        ticker.tick().await;
        let state = progress.borrow().clone();
        if state.transfer_id == 0 {
            continue;
        }

        info!(
            port,
            progress = %state.progress(),
            received = %state.received(),
            rate = %state.rate(),
            eta = %state.eta(),
            naks = state.naks,
            "multicast transfer progress"
        );
    }
}

pub async fn multicast_stream(
    port: u16,
) -> anyhow::Result<impl AsyncRead + Send + Unpin + 'static> {
    receiver_stream(local_ipv4()?, port).await
}

async fn receiver_stream(local_ip: Ipv4Addr, port: u16) -> anyhow::Result<MulticastStream> {
    let receiver = Receiver::builder()
        .socket(local_ip, MULTICAST_GROUP_ADDRESS, port)
        .map_err(|e| anyhow::anyhow!("failed to bind multicast receiver on port {port}: {e}"))?
        .max_wait(JOIN_TIMEOUT)
        .build();

    let reporter = AbortOnDropHandle::new(tokio::spawn(log_progress(port, receiver.progress())));

    let (session_pipe, pipe) = tokio::io::duplex(PIPE_CAPACITY);
    info!(port, "joining multicast group");
    let session = tokio::spawn(async move {
        let _reporter = reporter;
        match receiver.recv_to(session_pipe).await {
            Ok(summary) => info!(
                port,
                total_bytes = summary.total_bytes,
                blocks = summary.total_blocks,
                duplicates = summary.duplicates,
                late = summary.late,
                naks_sent = summary.naks_sent,
                loss = summary.loss(),
                "multicast transfer complete"
            ),
            Err(e) => warn!(port, error = %e, "multicast transfer failed"),
        }
    });

    Ok(MulticastStream {
        pipe,
        _session: AbortOnDropHandle::new(session),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use scuttlecast::sender::Sender;
    use tokio::io::AsyncReadExt;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_transfer_round_trips_every_byte_and_then_reports_eof() {
        const PORT: u16 = 50_902;
        const PAYLOAD_BYTES: usize = 512 * 1024;
        let payload: Vec<u8> = (0..PAYLOAD_BYTES).map(|i| (i % 251) as u8).collect();

        let mut stream = receiver_stream(Ipv4Addr::LOCALHOST, PORT)
            .await
            .expect("bind receiver");

        let sending = tokio::spawn({
            let payload = payload.clone();
            async move {
                Sender::builder()
                    .socket(Ipv4Addr::LOCALHOST, MULTICAST_GROUP_ADDRESS, PORT)
                    .expect("bind sender")
                    .min_receivers(1)
                    .build()
                    .send_stream(std::io::Cursor::new(payload), Some(PAYLOAD_BYTES as u64))
                    .await
                    .expect("send")
            }
        });

        let mut received = Vec::new();
        stream
            .read_to_end(&mut received)
            .await
            .expect("read to eof");
        sending.await.expect("send task");

        assert_eq!(received, payload);
    }
}
