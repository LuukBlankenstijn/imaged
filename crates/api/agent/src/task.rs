use axum::{Json, extract::Path};
use imaged_core::{
    di::{HostRepo, ImageRepo, MulticastMgr, Progress, TaskRepo},
    domain::task::{Task, TaskType},
};
use imaged_model::{
    agent::TaskFailure,
    error::{AppError, Result},
    task::TaskState,
};

use crate::AgentInfo;

#[injectable::inject(image_repo: ImageRepo, task_repo: TaskRepo, progress: Progress)]
pub async fn mark_finished(agent: AgentInfo, Path(task_id): Path<i64>) -> Result {
    let Some((task, host_id)) = task_to_settle(&agent.mac, task_id, TaskState::Done).await? else {
        return Ok(());
    };
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
    progress.forget(host_id);

    Ok(())
}

#[injectable::inject(image_repo: ImageRepo, task_repo: TaskRepo, multicast_mgr: MulticastMgr, progress: Progress)]
pub async fn mark_failed(
    agent: AgentInfo,
    Path(task_id): Path<i64>,
    Json(TaskFailure { error }): Json<TaskFailure>,
) -> Result {
    let Some((task, host_id)) = task_to_settle(&agent.mac, task_id, TaskState::Failed).await?
    else {
        return Ok(());
    };
    if task.task_type == TaskType::Capture
        && let Some(image_id) = task.image_id
    {
        image_repo.mark_faulted(image_id, &error).await?;
    }

    task_repo.mark_failed(task.id, host_id, &error).await?;
    progress.forget(host_id);
    if task.task_type == TaskType::Multicast {
        multicast_mgr.wake();
    }

    Ok(())
}

#[injectable::inject(host_repo: HostRepo, task_repo: TaskRepo)]
async fn task_to_settle(
    mac: &str,
    task_id: i64,
    outcome: TaskState,
) -> Result<Option<(Task, i64)>> {
    let host_id = host_repo.get_by_mac(mac).await?.id;
    if let Some(task) = task_repo.get_next(host_id).await?
        && task.id == task_id
    {
        return Ok(Some((task, host_id)));
    }
    let already_settled = match task_repo.get(task_id).await {
        Ok(task) => task.hosts_in(outcome).any(|id| id == host_id),
        Err(AppError::NotFound(_)) => false,
        Err(e) => return Err(e),
    };
    if already_settled {
        return Ok(None);
    }
    Err(AppError::InvalidArgument(format!(
        "Task {task_id} is not the next task for host {mac}",
    )))
}
