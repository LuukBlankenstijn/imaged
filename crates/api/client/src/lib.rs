pub mod model;

#[cfg(feature = "server")]
mod convert;

pub mod capture;
pub mod deploy;
pub mod event;
pub mod task;

#[cfg(feature = "server")]
mod agent;

#[cfg(feature = "server")]
pub use agent::AgentInfo;
#[cfg(feature = "server")]
use imaged_core::{
    di::{HostRepo, TaskRepo},
    domain::task::Task,
};
#[cfg(feature = "server")]
use imaged_shared::error::AppError;

#[cfg(feature = "server")]
#[injectable::inject(host_repo: HostRepo, task_repo: TaskRepo)]
async fn get_next_task(mac: &str) -> imaged_core::error::Result<Task> {
    let host = host_repo.get_by_mac(mac).await?;
    let task = task_repo
        .get_next(host.id)
        .await?
        .ok_or_else(|| AppError::InvalidArgument(format!("No active task found for host {mac}")))?;
    Ok(task)
}

#[cfg(all(test, feature = "server"))]
mod tests;
