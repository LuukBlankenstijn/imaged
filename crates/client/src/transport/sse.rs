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
    pub async fn disconnect(&self) {
        let url = match self.url("client/hosts/disconnect") {
            Ok(url) => url,
            Err(e) => {
                tracing::warn!(error = %e, "disconnect: failed to build url");
                return;
            }
        };
        if let Err(e) = self.send(self.client.post(url), "disconnect").await {
            tracing::warn!(error = %e, "failed to notify server of disconnect");
        }
    }

    pub async fn start_stream(
        &self,
        disk_size_bytes: u64,
    ) -> Result<impl Stream<Item = Result<StreamMessage>>> {
        let url = self.url(&format!(
            "client/hosts/stream?disk_size_bytes={}",
            disk_size_bytes
        ))?;

        let mut req = self
            .client
            .get(url)
            .header("X-Agent-Mac", &self.mac.to_string());
        if let Some(ip) = &self.ip {
            req = req.header("X-Agent-Ip", ip.to_string());
        }
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
