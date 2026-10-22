use axum::{
    Json,
    body::Body,
    extract::{Path, Query},
};
use bytes::Bytes;
use futures::TryStreamExt;
use imaged_core::{
    di::{HostRepo, ImageRepo, ImageSvc, TaskRepo},
    domain::task::{Task, TaskType},
};
use imaged_model::{
    agent::{ImagePartition, PartitionTableUpload, PartitionUpload},
    error::{AppError, Result},
    task::TaskState,
};

use crate::{AgentInfo, get_next_task};

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo)]
pub async fn upload_partition_data(
    agent: AgentInfo,
    Path((task_id, partition_number)): Path<(i64, i64)>,
    Query(PartitionUpload { filesystem, size }): Query<PartitionUpload>,
    body: Body,
) -> Result<Json<ImagePartition>> {
    let (task, image_id) = get_capture_task_and_verify(&agent.mac, task_id).await?;
    // Capture is single-host, so the aggregate equals that host's state.
    if task.aggregate_state() != TaskState::Running {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ));
    }
    let stream = body
        .into_data_stream()
        .map_err(|e| AppError::Internal(e.to_string()));

    image_service
        .save_partition_data(image_id, partition_number, stream)
        .await?;

    let partition = image_repo
        .save_partition(image_id, partition_number, &filesystem, size)
        .await?;

    Ok(Json(partition))
}

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo, host_repo: HostRepo, task_repo: TaskRepo)]
pub async fn partition_table(
    agent: AgentInfo,
    Path(task_id): Path<i64>,
    Query(PartitionTableUpload { sector_size }): Query<PartitionTableUpload>,
    body: Bytes,
) -> Result {
    let (task, image_id) = get_capture_task_and_verify(&agent.mac, task_id).await?;
    if task.aggregate_state() != TaskState::Pending {
        return Err(AppError::InvalidArgument(
            "Task has already stared".to_string(),
        ));
    }
    if sector_size < 512 || !sector_size.is_power_of_two() {
        return Err(AppError::InvalidArgument(format!(
            "invalid logical sector size {sector_size}"
        )));
    }

    image_service.clear_image_data(image_id).await?;
    // remove old partition records and marks image as capturing
    image_repo.start_capture(image_id, sector_size).await?;
    // save partition table
    image_service.save_partition_table(image_id, &body).await?;
    // mark this host's row running
    let host_id = host_repo.get_by_mac(&agent.mac).await?.id;
    task_repo.start(task.id, host_id).await?;

    Ok(())
}

#[injectable::inject(image_repo: ImageRepo)]
pub async fn root_partition(
    agent: AgentInfo,
    Path((task_id, partition_number)): Path<(i64, i64)>,
) -> Result {
    let (task, image_id) = get_capture_task_and_verify(&agent.mac, task_id).await?;
    if task.aggregate_state() != TaskState::Running {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ));
    }
    image_repo
        .set_root_partition(image_id, Some(partition_number))
        .await?;
    Ok(())
}

async fn get_capture_task_and_verify(mac: &str, task_id: i64) -> Result<(Task, i64)> {
    let task = get_next_task(mac).await?;
    let Some(image_id) = task.image_id else {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} is not valid"
        )));
    };
    if task.id != task_id || task.task_type != TaskType::Capture {
        Err(AppError::InvalidArgument(format!(
            "No capture task for task {task_id} found"
        )))
    } else {
        Ok((task, image_id))
    }
}
