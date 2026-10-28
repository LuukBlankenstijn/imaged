use std::fmt;
use std::path::Path;

use anyhow::{Result, ensure};
use imaged_disk::{Progress, grow, restore_compressed};
use imaged_model::agent::ImageLayout;
use tracing::info;

use super::{ClientTaskExt, Fresh, PARTCLONE_LOG, Shape, Source};
use crate::agent::Agent;
use crate::status::{Phase, Step};
use crate::sys::disk::{BlockDevice, PartitionTarget, find_target_disk};

#[derive(Clone)]
pub struct Restore<S, L, St = Fresh> {
    agent: Agent,
    task_id: i64,
    source: S,
    shape: L,
    state: St,
}

pub struct Prepared {
    disk: BlockDevice,
    layout: ImageLayout,
}

pub struct Planned {
    targets: Vec<PartitionTarget>,
}

pub struct Written {
    targets: Vec<PartitionTarget>,
}

impl<S, L> Restore<S, L> {
    pub fn new(agent: Agent, task_id: i64, source: S, shape: L) -> Self {
        Self {
            agent,
            task_id,
            source,
            shape,
            state: Fresh,
        }
    }
}

impl<S, L, St> Restore<S, L, St> {
    fn advance<Next>(self, next: impl FnOnce(St) -> Next) -> Restore<S, L, Next> {
        Restore {
            agent: self.agent,
            task_id: self.task_id,
            source: self.source,
            shape: self.shape,
            state: next(self.state),
        }
    }
}

impl<S: Source, L: Shape, St> fmt::Display for Restore<S, L, St> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} task {}", S::NAME, L::NAME, self.task_id)
    }
}

impl<S: Source, L: Shape> Restore<S, L, Fresh> {
    pub async fn prepare(self) -> Result<Restore<S, L, Prepared>> {
        self.agent.server.mark_started(self.task_id, None).await?;
        let layout = self.agent.server.image_layout(self.task_id).await?;
        ensure!(
            !layout.partitions.is_empty(),
            "image for task {} has no partitions to restore",
            self.task_id
        );
        let disk = find_target_disk().await?;
        Ok(self.advance(|Fresh| Prepared { disk, layout }))
    }
}

impl<S: Source, L: Shape> Restore<S, L, Prepared> {
    pub async fn plan(self) -> Result<Restore<S, L, Planned>> {
        self.agent
            .status
            .update_task(|task| task.step = Step::PartitionTable);
        let Restore {
            agent,
            task_id,
            source,
            shape,
            state: Prepared { disk, layout },
        } = self;
        let disk = shape
            .restore_table(&source, &agent, task_id, disk, &layout)
            .await?;
        let targets = shape.restore_plan(&disk, &layout)?;
        Ok(Restore {
            agent,
            task_id,
            source,
            shape,
            state: Planned { targets },
        })
    }
}

impl<S: Source, L: Shape> Restore<S, L, Planned> {
    pub async fn write(self) -> Result<Restore<S, L, Written>> {
        let status = &self.agent.status;
        let targets = &self.state.targets;
        let progress = status.partitions(targets);
        for (index, (target, progress)) in targets.iter().zip(progress).enumerate() {
            progress.report(Progress::default());
            let data = self
                .source
                .partition(&self.agent, self.task_id, target.number)
                .await?;
            info!(partition_number=%target.number, device=%target.device, filesystem=%target.filesystem, "restoring partition");
            let device = Path::new(&target.device);
            restore_compressed(
                target.filesystem,
                data,
                device,
                Path::new(PARTCLONE_LOG),
                |partition| progress.report(partition),
            )
            .await?;
            grow(target.filesystem, device).await?;
            status.update_task(|task| task.partitions[index].phase = Phase::Done);
        }
        status.update_task(|task| task.step = Step::Finishing);
        Ok(self.advance(|Planned { targets }| Written { targets }))
    }
}

impl<S: Source, L: Shape> Restore<S, L, Written> {
    pub async fn post_install(self) -> Result<()> {
        crate::task::post_install::run(&self.agent, self.task_id, &self.state.targets, L::FSTAB)
            .await
    }
}

impl<S: Source, L: Shape> ClientTaskExt for Restore<S, L> {
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
            .write()
            .await?
            .post_install()
            .await
    }

    async fn finalize(&self) -> Result<()> {
        super::finish_and_reboot(self).await
    }
}
