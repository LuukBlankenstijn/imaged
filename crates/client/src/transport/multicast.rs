use std::process::Stdio;

use imaged_shared::MULTICAST_RVD_ADDRESS;
use tokio::process::Command;

pub async fn udp_receiver_stream(
    port: u16,
) -> anyhow::Result<impl tokio::io::AsyncRead + Send + Unpin + 'static> {
    let rvd_address = MULTICAST_RVD_ADDRESS;
    let mut child = Command::new("udp-receiver")
        .args([
            "--portbase",
            &port.to_string(),
            "--nokbd",
            "--mcast-rdv-address",
            rvd_address,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("udp-receiver stdout missing"))?;

    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) if !status.success() => {
                tracing::error!(%status, "udp-receiver exited with error");
            }
            Err(e) => tracing::error!(err = %e, "failed to wait for udp-receiver"),
            _ => {}
        }
    });

    Ok(stdout)
}
