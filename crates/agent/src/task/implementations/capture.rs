use std::path::Path;

use derive_more::{Constructor, Display};
use imaged_disk::{backup_partition_table, clone_compressed, holds_system, root_partition};
use tracing::{info, warn};

use super::{ClientTaskExt, PARTCLONE_LOG};
use crate::agent::Agent;
use crate::status::PartitionProgress;
use crate::sys::disk::{PartitionTarget, repair_filesystem};
use crate::task::PARTTABLE_TMP;

#[derive(Constructor, Clone, Display)]
#[display("capture task {task_id}")]
pub struct CaptureTask {
    agent: Agent,
    task_id: i64,
}

impl ClientTaskExt for CaptureTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    fn agent(&self) -> &Agent {
        &self.agent
    }

    async fn handle_partition_table(&self, device: &str) -> anyhow::Result<()> {
        let table = backup_partition_table(Path::new(device), Path::new(PARTTABLE_TMP)).await?;

        let bytes = tokio::fs::read(PARTTABLE_TMP).await?;
        self.agent
            .server
            .upload_partition_table(self.task_id, table.sector_size, bytes.into())
            .await?;

        let _ = tokio::fs::remove_file(PARTTABLE_TMP).await;
        Ok(())
    }

    async fn handle_partition(
        &self,
        partition: PartitionTarget,
        progress: PartitionProgress,
    ) -> anyhow::Result<()> {
        repair_filesystem(&partition).await?;
        let compressed = clone_compressed(
            partition.filesystem,
            Path::new(&partition.device),
            Path::new(PARTCLONE_LOG),
            move |partition| progress.report(partition),
        )?;

        self.agent
            .server
            .upload_partition_data(
                self.task_id,
                partition.number,
                partition.filesystem.to_string(),
                partition.size as i64,
                compressed,
            )
            .await?;
        Ok(())
    }

    async fn after_partitions(&self, partitions: &[PartitionTarget]) -> anyhow::Result<()> {
        let mut systems = Vec::new();
        for p in partitions {
            if holds_system(p.number, p.filesystem, Path::new(&p.device)).await {
                systems.push(p.number);
            }
        }
        let root = root_partition(&systems);
        if let Some(root) = root {
            match self
                .agent
                .server
                .report_root_partition(self.task_id, root)
                .await
            {
                Ok(()) => info!(partition=%root, "reported the root partition"),
                Err(e) => warn!(partition=%root, error=%e, "could not report the root partition"),
            }
        }
        Ok(())
    }
}
