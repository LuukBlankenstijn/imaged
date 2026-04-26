use core::fmt;

use futures::{Stream, StreamExt as _};
use imaged_rpc::client::v1 as pb;
use imaged_rpc::client::v1::client_service_client::ClientServiceClient;

#[derive(Debug, thiserror::Error)]
pub enum StartStreamError {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("rpc failed: {0}")]
    Rpc(String),
}

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("stream error: {0}")]
    Transport(String),
}

#[derive(Debug)]
pub struct StreamMessage {}

impl fmt::Display for StreamMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "empty message")?;

        Ok(())
    }
}

impl From<pb::Task> for StreamMessage {
    fn from(_: pb::Task) -> Self {
        Self {}
    }
}

pub async fn start_stream(
    addr: String,
    mac: String,
    disk_size_bytes: u64,
) -> Result<impl Stream<Item = Result<StreamMessage, StreamError>>, StartStreamError> {
    let mut client = ClientServiceClient::connect(addr)
        .await
        .map_err(|e| StartStreamError::Connect(e.to_string()))?;

    let response = client
        .start_stream(pb::StartStreamRequest {
            mac,
            disk_size_bytes,
        })
        .await
        .map_err(|e| StartStreamError::Rpc(e.message().to_string()))?;

    let stream = response.into_inner().map(|item| {
        item.map(StreamMessage::from)
            .map_err(|status| StreamError::Transport(status.message().to_string()))
    });

    Ok(stream)
}
