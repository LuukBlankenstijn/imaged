use axum::{
    Json,
    extract::{Path, Query},
};
use imaged_core::{
    di::{HostRepo, ImageRepo, ImageSvc, MulticastMgr, Progress, TaskRepo},
    domain::task::{Task, TaskType},
};
use imaged_model::{
    agent::{TaskFailure, TaskStarted},
    error::{AppError, Result},
    image::ImageStatus,
    task::TaskState,
};

use crate::AgentInfo;

#[injectable::inject(host_repo: HostRepo, image_repo: ImageRepo, image_service: ImageSvc, task_repo: TaskRepo)]
pub async fn started(
    agent: AgentInfo,
    Path(task_id): Path<i64>,
    Query(TaskStarted { sector_size }): Query<TaskStarted>,
) -> Result {
    let host_id = host_repo.get_by_mac(&agent.mac).await?.id;
    let task = task_repo
        .get_next(host_id)
        .await?
        .filter(|task| task.id == task_id)
        .ok_or_else(|| {
            AppError::InvalidArgument(format!(
                "Task {task_id} is not the next task for host {}",
                agent.mac
            ))
        })?;
    let running = task.hosts_in(TaskState::Running).any(|id| id == host_id);
    if task.task_type == TaskType::Reboot {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} is not an imaging task"
        )));
    }
    let image_id = task
        .image_id
        .ok_or_else(|| AppError::InvalidArgument(format!("Task {task_id} has no image")))?;
    if task.task_type.captures() {
        if running {
            return Err(AppError::InvalidArgument(
                "Task has already started".to_string(),
            ));
        }
        let sector_size = sector_size.ok_or_else(|| {
            AppError::InvalidArgument("a capture must report its logical sector size".to_string())
        })?;
        if sector_size < 512 || !sector_size.is_power_of_two() {
            return Err(AppError::InvalidArgument(format!(
                "invalid logical sector size {sector_size}"
            )));
        }
        image_service.clear_image_data(image_id).await?;
        image_repo.start_capture(image_id, sector_size).await?;
    } else if image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    } else if task.task_type.multicasts() {
        if !running {
            return Err(AppError::InvalidArgument(format!(
                "Task {task_id} has not started on this host"
            )));
        }
        return Ok(());
    }
    task_repo.start(task.id, host_id).await
}

#[injectable::inject(image_repo: ImageRepo, image_service: ImageSvc, task_repo: TaskRepo, progress: Progress)]
pub async fn mark_finished(agent: AgentInfo, Path(task_id): Path<i64>) -> Result {
    let Some((task, host_id)) = task_to_settle(&agent.mac, task_id, TaskState::Done).await? else {
        return Ok(());
    };
    if task.task_type.captures()
        && let Some(image_id) = task.image_id
    {
        let partitions = image_repo.get_partitions(image_id).await?;
        let incomplete = if partitions.is_empty() {
            Some("Capture finished without any partitions")
        } else if task.task_type == TaskType::CapturePartition
            && !matches!(partitions.as_slice(), [only] if only.partition_number == 1)
        {
            Some("A partition capture must store exactly partition 1")
        } else if task.task_type == TaskType::Capture
            && !tokio::fs::try_exists(image_service.get_partition_table_path(image_id))
                .await
                .unwrap_or(false)
        {
            Some("Capture finished without a partition table")
        } else {
            None
        };
        if let Some(error) = incomplete {
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
    if task.task_type.captures()
        && let Some(image_id) = task.image_id
    {
        image_repo.mark_faulted(image_id, &error).await?;
    }

    task_repo.mark_failed(task.id, host_id, &error).await?;
    progress.forget(host_id);
    if task.task_type.multicasts() {
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
