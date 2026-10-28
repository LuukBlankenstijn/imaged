use anyhow::{Context as _, Result};
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::agent::Agent;
use crate::multicast::multicast_stream;

const PARTITION_TABLE_SLOT: i64 = 0;

#[allow(
    async_fn_in_trait,
    reason = "only awaited through the concrete sources held by the Task enum, whose futures are Send"
)]
pub trait Source: Clone {
    const NAME: &'static str;

    async fn partition_table(&self, agent: &Agent, task_id: i64) -> Result<Bytes>;

    async fn partition(
        &self,
        agent: &Agent,
        task_id: i64,
        number: i64,
    ) -> Result<impl AsyncRead + Unpin + Send>;
}

#[derive(Clone, Copy, Debug)]
pub struct Http;

impl Source for Http {
    const NAME: &'static str = "deploy";

    async fn partition_table(&self, agent: &Agent, task_id: i64) -> Result<Bytes> {
        agent.server.partition_table(task_id).await
    }

    async fn partition(
        &self,
        agent: &Agent,
        task_id: i64,
        number: i64,
    ) -> Result<impl AsyncRead + Unpin + Send> {
        agent.server.partition_data(task_id, number).await
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Multicast;

impl Source for Multicast {
    const NAME: &'static str = "multicast";

    async fn partition_table(&self, agent: &Agent, task_id: i64) -> Result<Bytes> {
        let mut table = Vec::new();
        multicast_stream(&agent.status, task_id, PARTITION_TABLE_SLOT)?
            .read_to_end(&mut table)
            .await
            .context("receiving the partition table")?;
        Ok(table.into())
    }

    async fn partition(
        &self,
        agent: &Agent,
        task_id: i64,
        number: i64,
    ) -> Result<impl AsyncRead + Unpin + Send> {
        multicast_stream(&agent.status, task_id, number)
    }
}
