pub mod capture;
pub mod reboot;
pub mod restore;
pub mod shape;
pub mod source;

pub use capture::Capture;
pub use reboot::RebootTask;
pub use restore::Restore;
pub use shape::{Shape, SinglePartition, WholeDisk};
pub use source::{Http, Multicast, Source};

use std::time::Duration;

use derive_more::Display;
use enum_dispatch::enum_dispatch;
use imaged_model::Work;

use crate::agent::Agent;
use crate::server::Rejected;
use crate::status::Outcome;

const PARTCLONE_LOG: &str = "/tmp/partclone-log";
const PARTTABLE_TMP: &str = "/parttable.bin";
const REPORT_RETRY_START: Duration = Duration::from_secs(1);
const REPORT_RETRY_CAP: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
pub struct Fresh;

#[allow(
    async_fn_in_trait,
    reason = "only awaited through concrete task types (the Task enum and test doubles), whose futures are Send"
)]
#[enum_dispatch]
pub trait ClientTaskExt: std::fmt::Display {
    fn task_id(&self) -> i64;

    fn agent(&self) -> &Agent;

    async fn run(&self) -> anyhow::Result<()>;

    async fn finalize(&self) -> anyhow::Result<()> {
        report_finished(self).await
    }

    async fn finalize_error(&self, err: &str) -> anyhow::Result<()> {
        tracing::error!(task=%self, error=%err, "did not finish task successfully");
        report(self, || {
            self.agent().server.mark_failed(self.task_id(), err)
        })
        .await?;
        self.agent()
            .status
            .task_ended(Outcome::Failed(err.to_string()));
        Ok(())
    }
}

async fn report<Report>(
    task: &(impl ClientTaskExt + ?Sized),
    send: impl Fn() -> Report,
) -> anyhow::Result<()>
where
    Report: Future<Output = anyhow::Result<()>>,
{
    let mut delay = REPORT_RETRY_START;
    loop {
        match send().await {
            Ok(()) => return Ok(()),
            Err(e) if Rejected::is_final(&e) => return Err(e),
            Err(e) => {
                tracing::warn!(task=%task, error=%format_args!("{e:#}"), retry_in=?delay, "could not report the result, retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(REPORT_RETRY_CAP);
            }
        }
    }
}

async fn report_finished(task: &(impl ClientTaskExt + ?Sized)) -> anyhow::Result<()> {
    report(task, || task.agent().server.mark_finished(task.task_id())).await?;
    task.agent().status.task_ended(Outcome::Done);
    tracing::info!(task=%task, "finished task successfully");
    Ok(())
}

async fn finish_and_reboot(task: &impl ClientTaskExt) -> anyhow::Result<()> {
    report_finished(task).await?;
    crate::reboot::after_countdown(task.agent()).await
}

#[derive(Display)]
#[display("{_variant}")]
#[enum_dispatch(ClientTaskExt)]
pub enum Task {
    Capture(Capture<WholeDisk>),
    CapturePartition(Capture<SinglePartition>),
    Deploy(Restore<Http, WholeDisk>),
    Multicast(Restore<Multicast, WholeDisk>),
    DeployPartition(Restore<Http, SinglePartition>),
    MulticastPartition(Restore<Multicast, SinglePartition>),
    Reboot(RebootTask),
}

impl Task {
    pub fn new(agent: Agent, task: &imaged_model::Task) -> Self {
        let id = task.id;
        let partition = |partuuid: &String| SinglePartition::new(partuuid.clone());
        match &task.work {
            Work::Capture { .. } => Capture::new(agent, id, WholeDisk).into(),
            Work::CapturePartition { partuuid, .. } => {
                Capture::new(agent, id, partition(partuuid)).into()
            }
            Work::Deploy { .. } => Restore::new(agent, id, Http, WholeDisk).into(),
            Work::Multicast { .. } => Restore::new(agent, id, Multicast, WholeDisk).into(),
            Work::DeployPartition { partuuid, .. } => {
                Restore::new(agent, id, Http, partition(partuuid)).into()
            }
            Work::MulticastPartition { partuuid, .. } => {
                Restore::new(agent, id, Multicast, partition(partuuid)).into()
            }
            Work::Reboot => RebootTask::new(agent, id).into(),
        }
    }
}
