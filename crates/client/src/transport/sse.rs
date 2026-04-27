use core::fmt;
use futures::{Stream, StreamExt as _};
use imaged_shared::ServerEvent;
use reqwest_eventsource::{Event, EventSource};
use std::convert::Infallible;

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("stream error: {0}")]
    Transport(String),
    #[error("decode error: {0}")]
    Decode(String),
}

#[derive(Debug)]
pub struct StreamMessage {
    pub event: ServerEvent,
}

impl fmt::Display for StreamMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.event)
    }
}

impl From<ServerEvent> for StreamMessage {
    fn from(event: ServerEvent) -> Self {
        Self { event }
    }
}

pub async fn start_stream(
    addr: String,
    mac: String,
    disk_size_bytes: u64,
) -> Result<impl Stream<Item = Result<StreamMessage, StreamError>>, Infallible> {
    let url = format!(
        "{}/client/hosts/stream?mac={}&disk_size_bytes={}",
        addr.trim_end_matches('/'),
        mac,
        disk_size_bytes
    );

    let es = EventSource::get(&url);

    let stream = es.filter_map(|event| async move {
        match event {
            Ok(Event::Open) => None, // skip open notifications
            Ok(Event::Message(msg)) => Some(
                serde_json::from_str::<ServerEvent>(&msg.data)
                    .map(StreamMessage::from)
                    .map_err(|e| StreamError::Decode(e.to_string())),
            ),
            Err(e) => Some(Err(StreamError::Transport(e.to_string()))),
        }
    });

    Ok(stream)
}
