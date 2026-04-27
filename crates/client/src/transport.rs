mod capture;
mod deploy;
pub mod sse;
mod task;

use reqwest::{Client, RequestBuilder, Response, Url};

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
}
