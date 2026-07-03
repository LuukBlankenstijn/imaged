use anyhow::Result;
use futures::{Stream, StreamExt as _};
use imaged_shared::ServerEvent;
use reqwest_eventsource::{Event, EventSource};

use crate::transport::ApiClient;

#[derive(Debug)]
pub struct StreamMessage {
    pub event: ServerEvent,
}

impl From<ServerEvent> for StreamMessage {
    fn from(event: ServerEvent) -> Self {
        Self { event }
    }
}

impl ApiClient {
    pub async fn start_stream(
        &self,
        disk_size_bytes: u64,
    ) -> Result<impl Stream<Item = Result<StreamMessage>>> {
        let url = self.url(&format!(
            "client/hosts/stream?disk_size_bytes={}",
            disk_size_bytes
        ))?;

        let req = self.client.get(url).header("X-Agent-Mac", &self.mac);
        let es = EventSource::new(req).expect("building eventsource");

        let stream = es.filter_map(|event| async move {
            match event {
                Ok(Event::Open) => None, // skip open notifications
                Ok(Event::Message(msg)) => Some(
                    serde_json::from_str::<ServerEvent>(&msg.data)
                        .map(StreamMessage::from)
                        .map_err(|e| anyhow::anyhow!("decode error: {e}")),
                ),
                Err(e) => Some(Err(anyhow::anyhow!("stream error: {e}"))),
            }
        });

        Ok(stream)
    }
}
