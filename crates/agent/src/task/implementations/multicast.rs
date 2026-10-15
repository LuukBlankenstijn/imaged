use std::path::Path;
use std::sync::Arc;

use derive_more::{Constructor, Display};
use imaged_disk::{restore_compressed, restore_partition_table};
use tokio::io::AsyncReadExt;
use tracing::{debug, info};

use super::{ClientTaskExt, PARTCLONE_LOG};
use crate::progress::PartitionProgress;
use crate::server::Server;
use crate::sys::disk::{BlockDevice, PartitionTarget};
use crate::{multicast::multicast_stream, task::PARTTABLE_TMP};

#[derive(Clone, Display, Constructor)]
#[display("multicast task")]
pub struct MulticastTask {
    server: Arc<Server>,
    task_id: i64,
}

impl ClientTaskExt for MulticastTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    fn server(&self) -> &Server {
        &self.server
    }

    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        let mut buffer: Vec<u8> = Vec::new();
        multicast_stream(self.task_id, 0)?
            .read_to_end(&mut buffer)
            .await
            .map_err(|e| anyhow::anyhow!("failed to receive the partition table: {e}"))?;
        tokio::fs::write(PARTTABLE_TMP, buffer).await?;
        restore_partition_table(Path::new(device), Path::new(PARTTABLE_TMP)).await?;

        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        Ok(())
    }

    async fn plan_partitions(&self, disk: &BlockDevice) -> anyhow::Result<Vec<PartitionTarget>> {
        super::image_partitions(&self.server, self.task_id, disk).await
    }

    async fn handle_partition(
        &self,
        partition: PartitionTarget,
        progress: PartitionProgress,
    ) -> anyhow::Result<()> {
        debug!(partition_number=%partition.number, "starting partition download over multicast");
        let stream = multicast_stream(self.task_id, partition.number)?;

        info!(partition_number=%partition.number, filesystem=%partition.filesystem, "restoring partition");
        restore_compressed(
            partition.filesystem,
            stream,
            Path::new(&partition.device),
            Path::new(PARTCLONE_LOG),
            |partition| progress.report(partition),
        )
        .await
    }

    async fn after_partitions(&self, partitions: &[PartitionTarget]) -> anyhow::Result<()> {
        crate::task::post_install::run(&self.server, self.task_id, partitions).await
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        super::finish_and_reboot(self).await
    }
}
