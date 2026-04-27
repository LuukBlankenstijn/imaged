use reqwest::Body;
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;

use crate::transport::ApiClient;

impl ApiClient {
    pub async fn upload_parttable(&self, image_id: i64, data: Vec<u8>) -> anyhow::Result<()> {
        let url = self.url(&format!("client/images/{}/parttable", image_id))?;
        self.send(self.client.put(url).body(data), "upload_parttable")
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
