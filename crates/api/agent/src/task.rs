use axum::{Json, extract::Path};
use imaged_core::{
    di::{HostRepo, ImageRepo, MulticastMgr, TaskRepo},
    domain::task::TaskType,
};
use imaged_model::{
    agent::TaskFailure,
    error::{AppError, Result},
};

use crate::{AgentInfo, get_next_task};

#[injectable::inject(host_repo: HostRepo, image_repo: ImageRepo, task_repo: TaskRepo)]
pub async fn mark_finished(agent: AgentInfo, Path(task_id): Path<i64>) -> Result {
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
        if image_repo.get_partitions(image_id).await?.is_empty() {
            let error = "Capture finished without any partitions";
            image_repo.mark_faulted(image_id, error).await?;
            task_repo.mark_failed(task.id, host_id, error).await?;
            return Err(AppError::FailedPrecondition(error.to_string()));
        }
        image_repo.mark_finished(image_id).await?;
    }
    task_repo.mark_finished(task.id, host_id).await?;

    Ok(())
}

#[injectable::inject(host_repo: HostRepo, image_repo: ImageRepo, task_repo: TaskRepo, multicast_mgr: MulticastMgr)]
pub async fn mark_failed(
    agent: AgentInfo,
    Path(task_id): Path<i64>,
    Json(TaskFailure { error }): Json<TaskFailure>,
) -> Result {
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
    if task.task_type == TaskType::Multicast {
        multicast_mgr.wake();
    }

    Ok(())
}
