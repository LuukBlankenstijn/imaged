use std::io;

use futures::TryStreamExt;
use tokio::io::AsyncRead;
use tokio_util::io::StreamReader;

use crate::transport::ApiClient;

impl ApiClient {
    pub async fn download_parttable(&self, image_id: i64) -> anyhow::Result<Vec<u8>> {
        let url = self.url(&format!("client/images/{}/parttable", image_id))?;
        let response = self
            .send(self.client.get(url), "download_parttable")
            .await?;
        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }

    pub async fn download_partition_data(
        &self,
        image_id: i64,
        partition_number: i64,
    ) -> anyhow::Result<impl AsyncRead + Send + Sync + 'static> {
        let url = self.url(&format!(
            "client/images/{}/partitions/{}/data",
            image_id, partition_number
        ))?;

        let req = self.client.get(url);
        let response = self.send(req, "download_partition_data").await?;

        let stream = response.bytes_stream().map_err(io::Error::other);

        Ok(StreamReader::new(stream))
    }
}
