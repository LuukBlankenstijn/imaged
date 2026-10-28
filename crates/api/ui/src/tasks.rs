use dioxus::prelude::*;

use crate::model::{KickRequest, ScriptRun, TaskPage, TaskQuery};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use crate::model::TaskStatus;
#[cfg(feature = "server")]
use imaged_core::di::{ImageRepo, MulticastMgr, Progress, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::{Task as DTask, TaskQuery as DTaskQuery};
#[cfg(feature = "server")]
use imaged_model::error::AppError;
use imaged_model::error::Result;
#[cfg(feature = "server")]
use imaged_model::task::TaskState;

#[cfg(feature = "server")]
const DEFAULT_PAGE_SIZE: u32 = 50;

#[get("/api/ui/tasks?{query}")]
#[inject(task_repo: TaskRepo)]
pub async fn list_tasks(query: TaskQuery) -> Result<TaskPage> {
    let page = task_repo
        .list(&DTaskQuery {
            states: query.status.map(status_states).unwrap_or_default(),
            task_types: query
                .task_type
                .map(|task_type| task_type.stored_types())
                .unwrap_or_default(),
            host_id: query.host_id,
            before: query.before,
            limit: query.limit.unwrap_or(DEFAULT_PAGE_SIZE),
        })
        .await?;
    Ok(page.into())
}

#[cfg(feature = "server")]
fn status_states(status: TaskStatus) -> Vec<TaskState> {
    match status {
        TaskStatus::Active => vec![TaskState::Pending, TaskState::Running],
        TaskStatus::Done => vec![TaskState::Done],
        TaskStatus::Failed => vec![TaskState::Failed, TaskState::Partial],
        TaskStatus::Cancelled => vec![TaskState::Cancelled],
    }
}

#[get("/api/ui/tasks/{task_id}/hosts/{host_id}/scripts")]
#[inject(task_repo: TaskRepo)]
pub async fn get_script_runs(task_id: i64, host_id: i64) -> Result<Vec<ScriptRun>> {
    let runs = task_repo.get_script_runs(task_id, host_id).await?;
    Ok(runs.into_iter().map(Into::into).collect())
}

#[post("/api/ui/tasks/cancel")]
#[inject(task_repo: TaskRepo)]
pub async fn cancel_task(id: i64) -> Result<()> {
    let task = task_repo.get(id).await?;
    let state = task.aggregate_state();
    if !(state.is_pending() || state.is_running()) {
        return Err(AppError::InvalidArgument(format!(
            "cannot cancel task {id}, task is not pending or running"
        )));
    }
    cancel_active(&task).await
}

#[cfg(feature = "server")]
#[inject(task_repo: TaskRepo, image_repo: ImageRepo, multicast_mgr: MulticastMgr, registry: Registry, progress: Progress)]
pub(crate) async fn cancel_active(task: &DTask) -> Result<()> {
    task_repo.cancel(task.id).await?;
    if let Some(image_id) = task.image_id
        && task.task_type.captures()
    {
        image_repo
            .mark_faulted(image_id, "Capture task was cancelled by user")
            .await?;
    }
    if task.task_type.multicasts() {
        multicast_mgr.wake();
    }
    for host in task
        .hosts
        .iter()
        .filter(|h| h.state.is_pending() || h.state.is_running())
    {
        registry.dispatch(host.host_id);
        progress.forget(host.host_id);
    }
    Ok(())
}

#[post("/api/ui/tasks/kick")]
#[inject(task_repo: TaskRepo, multicast_mgr: MulticastMgr, registry: Registry, progress: Progress)]
pub async fn kick_host(req: KickRequest) -> Result<()> {
    let KickRequest { task_id, host_id } = req;
    let task = task_repo.get(task_id).await?;
    if !task.task_type.multicasts() || !task.hosts_in(TaskState::Running).any(|h| h == host_id) {
        return Err(AppError::InvalidArgument(format!(
            "cannot kick host {host_id} from task {task_id}, host is not receiving the multicast"
        )));
    }
    task_repo
        .mark_failed(task.id, host_id, "Kicked from the multicast by user")
        .await?;
    multicast_mgr.wake();
    registry.dispatch(host_id);
    progress.forget(host_id);
    Ok(())
}

#[post("/api/ui/tasks/retry")]
#[inject(task_repo: TaskRepo, multicast_mgr: MulticastMgr, registry: Registry)]
pub async fn retry_task(id: i64) -> Result<()> {
    let task = task_repo.get(id).await?;
    let state = task.aggregate_state();
    if !(state.is_cancelled() || state.is_failed() || state.is_partial()) {
        return Err(AppError::InvalidArgument(format!(
            "cannot retry task {id}, task has no failed or cancelled hosts"
        )));
    }
    if task.hosts.is_empty() {
        return Err(AppError::InvalidArgument(format!(
            "cannot retry task {id}, hosts are deleted"
        )));
    }
    if task.image_id.is_some() && task.image_deleted {
        return Err(AppError::InvalidArgument(format!(
            "cannot retry task {id}, image is deleted"
        )));
    }
    task_repo.retry(task.id).await?;
    if task.task_type.multicasts() {
        multicast_mgr.wake();
        return Ok(());
    }
    for host in &task.hosts {
        registry.dispatch(host.host_id);
    }
    Ok(())
}
