pub mod sse;

use reqwest::{Body, Client, RequestBuilder, Response, Url};
use serde_json::json;
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    base: Url,
    mac: String,
}

impl ApiClient {
    pub fn new(base: String, mac: String) -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::new(),
            base: Url::parse(&base)?,
            mac,
        })
    }

    fn url(&self, path: &str) -> anyhow::Result<Url> {
        Ok(self.base.join(path)?)
    }

    async fn send(&self, builder: RequestBuilder, context: &str) -> anyhow::Result<Response> {
        let response = builder.header("X-Agent-Mac", &self.mac).send().await?;

        if let Err(e) = response.error_for_status_ref() {
            let status = response.status();
            let body = response.text().await.unwrap_or_else(|_| "<no body>".into());
            tracing::error!(%status, body=%body, context, "request failed");
            return Err(e.into());
        }

        Ok(response)
    }

    pub async fn upload_parttable(&self, image_id: i64, data: Vec<u8>) -> anyhow::Result<()> {
        let url = self.url(&format!("client/images/{}/parttable", image_id))?;
        self.send(self.client.put(url).body(data), "upload_parttable")
            .await?;
        Ok(())
    }

    pub async fn mark_capture_finished(&self, image_id: i64) -> anyhow::Result<()> {
        let url = self.url(&format!("client/images/{}/finished", image_id))?;

        self.send(self.client.post(url), "mark_capture_finished")
            .await?;
        Ok(())
    }

    pub async fn mark_capture_failed(&self, image_id: i64, err: String) -> anyhow::Result<()> {
        let url = self.url(&format!("client/images/{}/failed", image_id))?;

        self.send(
            self.client.post(url).json(&json!({
                "error": err
            })),
            "mark_capture_failed",
        )
        .await?;
        Ok(())
    }

    pub async fn upload_partition_data<R>(
        &self,
        image_id: i64,
        partition_number: i64,
        fstype: &str,
        part_size: u64,
        reader: R,
    ) -> anyhow::Result<()>
    where
        R: AsyncRead + Send + Sync + 'static,
    {
        let url = self.url(&format!(
            "client/images/{}/partitions/{}/data",
            image_id, partition_number
        ))?;
        let stream = ReaderStream::new(reader);

        let req = self
            .client
            .put(url)
            .header("X-Fstype", fstype)
            .header("X-Partition-Size", part_size)
            .body(Body::wrap_stream(stream));

        self.send(req, "upload_partition_data").await?;
        Ok(())
    }
}
