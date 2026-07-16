use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};

use imaged_shared::MULTICAST_RVD_ADDRESS;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::process::{Child, ChildStdout, Command};

struct UdpReceiverStream {
    stdout: ChildStdout,
    _child: Child,
}

impl AsyncRead for UdpReceiverStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stdout).poll_read(cx, buf)
    }
}

pub async fn udp_receiver_stream(
    port: u16,
) -> anyhow::Result<impl AsyncRead + Send + Unpin + 'static> {
    let rvd_address = MULTICAST_RVD_ADDRESS;
    let mut cmd = Command::new("udp-receiver");
    cmd.args([
        "--portbase",
        &port.to_string(),
        "--nokbd",
        "--mcast-rdv-address",
        rvd_address,
    ]);
    spawn_receiver(cmd).await
}

async fn spawn_receiver(mut cmd: Command) -> anyhow::Result<UdpReceiverStream> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("udp-receiver stdout missing"))?;

    Ok(UdpReceiverStream {
        stdout,
        _child: child,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_kills_receiver_child() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let marker =
            std::env::temp_dir().join(format!("imaged-drop-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_file(&marker);

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep 1 && touch {}", marker.display()));

        let stream = spawn_receiver(cmd).await.expect("spawn stub receiver");
        drop(stream);

        std::thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "receiver child survived drop and created {}",
            marker.display()
        );
        let _ = std::fs::remove_file(&marker);
    }
}
