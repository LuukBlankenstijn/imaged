use dioxus_fullstack::*;
use imaged_shared::error::Result;

#[cfg(feature = "server")]
use imaged_shared::error::AppError;

#[cfg(feature = "server")]
use imaged_core::{
    di::{HostRepo, ImageRepo, TaskRepo},
    domain::task::TaskType,
};

#[cfg(feature = "server")]
use crate::{AgentInfo, get_next_task};

#[injectable::inject(host_repo: HostRepo, image_repo: ImageRepo, task_repo: TaskRepo)]
#[post("/api/client/tasks/{task_id}/finished", agent: AgentInfo)]
pub async fn mark_finished(task_id: i64) -> Result {
    let mac = &agent.mac;
    let task = get_next_task(mac).await?;
    if task.id != task_id {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} is not the next task for host {mac}",
        )));
    }
    let host_id = host_repo.get_by_mac(mac).await?.id;
    if task.task_type == TaskType::Capture
        && let Some(image_id) = task.image_id
    {
        image_repo.mark_finished(image_id).await?;
    }
    task_repo.mark_finished(task.id, host_id).await?;

    Ok(())
}

#[injectable::inject(host_repo: HostRepo, image_repo: ImageRepo, task_repo: TaskRepo)]
#[post("/api/client/tasks/{task_id}/failed", agent: AgentInfo)]
pub async fn mark_failed(task_id: i64, error: String) -> Result {
    let mac = &agent.mac;
    let task = get_next_task(mac).await?;
    if task.id != task_id {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} is not the next task for host {mac}",
        )));
    }
    let host_id = host_repo.get_by_mac(mac).await?.id;
    if task.task_type == TaskType::Capture
        && let Some(image_id) = task.image_id
    {
        image_repo.mark_faulted(image_id, &error).await?;
    }

    task_repo.mark_failed(task.id, host_id, &error).await?;

    Ok(())
}
