use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use futures::stream;
use imaged_shared::{MULTICAST_JOIN_WINDOW, get_multicast_port, multicast_group};
use scuttlecast::receiver::Receiver;
use scuttlecast::state::ReceiveState;
use tokio::io::AsyncBufRead;
use tokio::sync::watch;
use tokio_util::io::StreamReader;
use tokio_util::task::AbortOnDropHandle;
use tracing::{info, warn};

use crate::sys;

const JOIN_TIMEOUT: Duration = MULTICAST_JOIN_WINDOW.saturating_add(Duration::from_secs(60));
const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(2);

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

pub fn multicast_stream(
    task_id: i64,
    slot: i64,
) -> anyhow::Result<impl AsyncBufRead + Send + Unpin + 'static> {
    receiver_stream(
        local_ipv4()?,
        multicast_group(task_id),
        get_multicast_port(slot),
    )
}

fn receiver_stream(
    local_ip: Ipv4Addr,
    group: Ipv4Addr,
    port: u16,
) -> anyhow::Result<impl AsyncBufRead + Send + Unpin + 'static> {
    let receiver = Receiver::builder()
        .socket(local_ip, group, port)
        .map_err(|e| anyhow::anyhow!("failed to bind multicast receiver on {group}:{port}: {e}"))?
        .max_wait(JOIN_TIMEOUT)
        .build();
    let reporter = AbortOnDropHandle::new(tokio::spawn(log_progress(port, receiver.progress())));
    info!(%group, port, "joining multicast group");

    let blocks = stream::unfold(
        Some((receiver.recv_stream(), reporter)),
        move |state| async move {
            let (mut transfer, reporter) = state?;
            if let Some(block) = transfer.recv().await {
                return Some((Ok(block), Some((transfer, reporter))));
            }
            match transfer.finish().await {
                Ok(summary) => {
                    info!(
                        port,
                        total_bytes = summary.total_bytes,
                        blocks = summary.total_blocks,
                        duplicates = summary.duplicates,
                        late = summary.late,
                        naks_sent = summary.naks_sent,
                        loss = summary.loss(),
                        "multicast transfer complete"
                    );
                    None
                }
                Err(e) => {
                    warn!(port, error = %e, "multicast transfer failed");
                    Some((Err(io::Error::other(e)), None))
                }
            }
        },
    );
    Ok(StreamReader::new(Box::pin(blocks)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scuttlecast::sender::Sender;
    use tokio::io::AsyncReadExt;

    const GROUP: Ipv4Addr = Ipv4Addr::new(239, 192, 0, 1);

    fn sender(port: u16) -> Sender {
        Sender::builder()
            .socket(Ipv4Addr::LOCALHOST, GROUP, port)
            .expect("bind sender")
            .min_receivers(1)
            .max_rate(200.0)
            .build()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_transfer_round_trips_every_byte_and_then_reports_eof() {
        const PORT: u16 = 50_902;
        const PAYLOAD_BYTES: usize = 128 * 1024;
        let payload: Vec<u8> = (0..PAYLOAD_BYTES).map(|i| (i % 251) as u8).collect();

        let mut stream = receiver_stream(Ipv4Addr::LOCALHOST, GROUP, PORT).expect("bind receiver");
        let sending = sender(PORT).send_stream(
            std::io::Cursor::new(payload.clone()),
            Some(PAYLOAD_BYTES as u64),
        );

        let mut received = Vec::new();
        stream
            .read_to_end(&mut received)
            .await
            .expect("read to eof");
        sending.await.expect("send");

        assert_eq!(received, payload);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_kicked_receiver_fails_the_read_instead_of_ending_cleanly() {
        const PORT: u16 = 50_906;
        let payload = vec![0xA5u8; 4 * 1024 * 1024];
        let announced = payload.len() as u64;

        let mut stream = receiver_stream(Ipv4Addr::LOCALHOST, GROUP, PORT).expect("bind receiver");
        let sending = sender(PORT).send_stream(std::io::Cursor::new(payload), Some(announced));
        let kicker = sending.kicker();
        let mut progress = sending.progress();
        let joined = progress
            .wait_for(|state| !state.receivers.is_empty())
            .await
            .expect("progress")
            .receivers[0]
            .receiver_id;

        assert!(kicker.kick(joined));
        let mut received = Vec::new();
        let read = stream.read_to_end(&mut received).await;

        assert!(read.is_err(), "a kicked transfer read as a clean EOF");
        assert!((received.len() as u64) < announced);
    }
}
