use std::io;
use std::path::Path;

use derive_more::{Constructor, Display};
use futures::StreamExt;
use imaged_disk::{restore_compressed, restore_partition_table};
use tokio_util::io::StreamReader;
use tracing::{debug, info};

use super::{ClientTaskExt, PARTCLONE_LOG};
use crate::sys::disk::{BlockDevice, PartitionTarget};
use crate::task::PARTTABLE_TMP;

#[derive(Constructor, Clone, Display)]
#[display("deploy task {task_id}")]
pub struct DeployTask {
    task_id: i64,
}

impl ClientTaskExt for DeployTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        let data = api::deploy::download_partition_table(self.task_id).await?;
        tokio::fs::write(PARTTABLE_TMP, data).await?;
        restore_partition_table(Path::new(device), Path::new(PARTTABLE_TMP)).await?;

        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        Ok(())
    }

    async fn plan_partitions(&self, disk: &BlockDevice) -> anyhow::Result<Vec<PartitionTarget>> {
        super::image_partitions(self.task_id, disk).await
    }

    async fn handle_partition(&self, partition: PartitionTarget) -> anyhow::Result<()> {
        debug!(partition_number=%partition.number, "starting partition download");
        let stream = api::deploy::download_partition_data(self.task_id, partition.number).await?;
        let compressed = StreamReader::new(
            stream
                .into_inner()
                .map(|item| item.map_err(io::Error::other)),
        );

        info!(partition_number=%partition.number, filesystem=%partition.filesystem, "restoring partition");
        restore_compressed(
            partition.filesystem,
            compressed,
            Path::new(&partition.device),
            Path::new(PARTCLONE_LOG),
        )
        .await
    }

    async fn after_partitions(&self, partitions: &[PartitionTarget]) -> anyhow::Result<()> {
        crate::task::post_install::run(self.task_id, partitions).await
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        super::finish_and_reboot(self).await
    }
}
