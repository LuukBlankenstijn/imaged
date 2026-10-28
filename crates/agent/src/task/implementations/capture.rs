use std::fmt;
use std::path::Path;

use anyhow::Result;
use imaged_disk::{Progress, clone_compressed, holds_system, root_partition};
use imaged_model::agent::PartitionUpload;
use tracing::{info, warn};

use super::{ClientTaskExt, Fresh, PARTCLONE_LOG, Shape};
use crate::agent::Agent;
use crate::status::{Phase, Step};
use crate::sys::disk::{BlockDevice, PartitionTarget, find_target_disk, repair_filesystem};

#[derive(Clone)]
pub struct Capture<L, St = Fresh> {
    agent: Agent,
    task_id: i64,
    shape: L,
    state: St,
}

pub struct Prepared {
    disk: BlockDevice,
}

pub struct Planned {
    targets: Vec<PartitionTarget>,
}

pub struct Uploaded {
    targets: Vec<PartitionTarget>,
}

impl<L> Capture<L> {
    pub fn new(agent: Agent, task_id: i64, shape: L) -> Self {
        Self {
            agent,
            task_id,
            shape,
            state: Fresh,
        }
    }
}

impl<L, St> Capture<L, St> {
    fn advance<Next>(self, next: impl FnOnce(St) -> Next) -> Capture<L, Next> {
        Capture {
            agent: self.agent,
            task_id: self.task_id,
            shape: self.shape,
            state: next(self.state),
        }
    }
}

impl<L: Shape, St> fmt::Display for Capture<L, St> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "capture {} task {}", L::NAME, self.task_id)
    }
}

impl<L: Shape> Capture<L, Fresh> {
    pub async fn prepare(self) -> Result<Capture<L, Prepared>> {
        let disk = find_target_disk().await?;
        let sector_size = disk.logical_sector_size()?;
        self.agent
            .server
            .mark_started(self.task_id, Some(sector_size))
            .await?;
        Ok(self.advance(|Fresh| Prepared { disk }))
    }
}

impl<L: Shape> Capture<L, Prepared> {
    pub async fn plan(self) -> Result<Capture<L, Planned>> {
        self.agent
            .status
            .update_task(|task| task.step = Step::PartitionTable);
        let disk = &self.state.disk;
        self.shape
            .capture_table(&self.agent, self.task_id, disk)
            .await?;
        let targets = self.shape.capture_plan(disk)?;
        Ok(self.advance(|_| Planned { targets }))
    }
}

impl<L: Shape> Capture<L, Planned> {
    pub async fn upload(self) -> Result<Capture<L, Uploaded>> {
        let status = &self.agent.status;
        let targets = &self.state.targets;
        let progress = status.partitions(targets);
        for (index, (target, progress)) in targets.iter().zip(progress).enumerate() {
            progress.report(Progress::default());
            repair_filesystem(target).await?;
            let compressed = clone_compressed(
                target.filesystem,
                Path::new(&target.device),
                Path::new(PARTCLONE_LOG),
                move |partition| progress.report(partition),
            )?;
            let upload = PartitionUpload {
                filesystem: target.filesystem.to_string(),
                size: target.size as i64,
                fs_uuid: target.fs_uuid.clone(),
                fs_label: target.fs_label.clone(),
            };
            self.agent
                .server
                .upload_partition_data(self.task_id, target.number, &upload, compressed)
                .await?;
            status.update_task(|task| task.partitions[index].phase = Phase::Done);
        }
        status.update_task(|task| task.step = Step::Finishing);
        Ok(self.advance(|Planned { targets }| Uploaded { targets }))
    }
}

impl<L: Shape> Capture<L, Uploaded> {
    pub async fn report_root(self) -> Result<()> {
        let mut systems = Vec::new();
        for target in &self.state.targets {
            if holds_system(target.number, target.filesystem, Path::new(&target.device)).await {
                systems.push(target.number);
            }
        }
        if let Some(root) = root_partition(&systems) {
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

impl<L: Shape> ClientTaskExt for Capture<L> {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    fn agent(&self) -> &Agent {
        &self.agent
    }

    async fn run(&self) -> Result<()> {
        self.clone()
            .prepare()
            .await?
            .plan()
            .await?
            .upload()
            .await?
            .report_root()
            .await
    }
}
