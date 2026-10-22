use std::path::Path;

use derive_more::{Constructor, Display};
use imaged_disk::{restore_compressed, restore_partition_table};
use tracing::{debug, info};

use super::{ClientTaskExt, PARTCLONE_LOG};
use crate::agent::Agent;
use crate::status::PartitionProgress;
use crate::sys::disk::{BlockDevice, PartitionTarget};
use crate::task::PARTTABLE_TMP;

#[derive(Constructor, Clone, Display)]
#[display("deploy task {task_id}")]
pub struct DeployTask {
    agent: Agent,
    task_id: i64,
}

impl ClientTaskExt for DeployTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    fn agent(&self) -> &Agent {
        &self.agent
    }

    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        let data = self.agent.server.partition_table(self.task_id).await?;
        let layout = self.agent.server.image_layout(self.task_id).await?;
        tokio::fs::write(PARTTABLE_TMP, data).await?;
        restore_partition_table(
            Path::new(device),
            Path::new(PARTTABLE_TMP),
            layout.sector_size,
        )
        .await?;

        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        Ok(())
    }

    async fn plan_partitions(&self, disk: &BlockDevice) -> anyhow::Result<Vec<PartitionTarget>> {
        super::image_partitions(&self.agent, self.task_id, disk).await
    }

    async fn handle_partition(
        &self,
        partition: PartitionTarget,
        progress: PartitionProgress,
    ) -> anyhow::Result<()> {
        debug!(partition_number=%partition.number, "starting partition download");
        let compressed = self
            .agent
            .server
            .partition_data(self.task_id, partition.number)
            .await?;

        info!(partition_number=%partition.number, filesystem=%partition.filesystem, "restoring partition");
        restore_compressed(
            partition.filesystem,
            compressed,
            Path::new(&partition.device),
            Path::new(PARTCLONE_LOG),
            |partition| progress.report(partition),
        )
        .await
    }

    async fn after_partitions(&self, partitions: &[PartitionTarget]) -> anyhow::Result<()> {
        crate::task::post_install::run(&self.agent, self.task_id, partitions).await
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        super::finish_and_reboot(self).await
    }
}
