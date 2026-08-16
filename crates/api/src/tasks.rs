use dioxus::prelude::*;

use crate::model::Task;

#[cfg(feature = "server")]
use inject::inject;

#[cfg(feature = "server")]
use crate::error::sfe;
#[cfg(feature = "server")]
use imaged_server_core::di::{ImageRepo, MulticastMgr, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_server_core::domain::task::TaskType;
#[cfg(feature = "server")]
use imaged_server_core::error::AppError;

#[get("/api/ui/tasks")]
#[inject(task_repo: TaskRepo)]
pub async fn get_all_tasks() -> ServerFnResult<Vec<Task>> {
    let tasks = task_repo.get_all().await.map_err(sfe)?;
    Ok(tasks.into_iter().map(Into::into).collect())
}

#[post("/api/ui/tasks/cancel")]
#[inject(task_repo: TaskRepo, image_repo: ImageRepo, multicast_mgr: MulticastMgr, registry: Registry)]
pub async fn cancel_task(id: i64) -> ServerFnResult<()> {
    let task = task_repo.get(id).await.map_err(sfe)?;
    let state = task.aggregate_state();
    if !(state.is_pending() || state.is_running()) {
        return Err(sfe(AppError::InvalidArgument(format!(
            "cannot cancel task {id}, task is not pending or running"
        ))));
    }
    task_repo.cancel(task.id).await.map_err(sfe)?;
    if let Some(image_id) = task.image_id
        && task.task_type == TaskType::Capture
    {
        image_repo
            .mark_faulted(image_id, "Capture task was cancelled by user")
            .await
            .map_err(sfe)?;
    }
    if task.task_type == TaskType::Multicast {
        multicast_mgr.cancel(task.id);
    }
    for host in task
        .hosts
        .iter()
        .filter(|h| h.state.is_pending() || h.state.is_running())
    {
        registry.cancel_task(host.host_id, task.id);
    }
    Ok(())
}

#[post("/api/ui/tasks/retry")]
#[inject(task_repo: TaskRepo, multicast_mgr: MulticastMgr, registry: Registry)]
pub async fn retry_task(id: i64) -> ServerFnResult<()> {
    let task = task_repo.get(id).await.map_err(sfe)?;
    let state = task.aggregate_state();
    if !(state.is_cancelled() || state.is_failed() || state.is_partial()) {
        return Err(sfe(AppError::InvalidArgument(format!(
            "cannot retry task {id}, task has no failed or cancelled hosts"
        ))));
    }
    if task.hosts.is_empty() {
        return Err(sfe(AppError::InvalidArgument(format!(
            "cannot retry task {id}, hosts are deleted"
        ))));
    }
    if task.image_id.is_some() && task.image_deleted {
        return Err(sfe(AppError::InvalidArgument(format!(
            "cannot retry task {id}, image is deleted"
        ))));
    }
    task_repo.retry(task.id).await.map_err(sfe)?;
    if task.task_type == TaskType::Multicast {
        multicast_mgr.notify_new(task.id).map_err(sfe)?;
    }
    for host in &task.hosts {
        if let Some(next_task) = task_repo.get_next(host.host_id).await.map_err(sfe)?
            && next_task.id == task.id
        {
            registry.send_task(host.host_id, &task);
        }
    }
    Ok(())
}
