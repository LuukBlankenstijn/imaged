use axum::{
    Json,
    body::Body,
    extract::{Path, Query},
};
use bytes::Bytes;
use futures::TryStreamExt;
use imaged_core::{
    di::{ImageRepo, ImageSvc},
    domain::task::{Task, TaskType},
};
use imaged_model::{
    agent::{ImagePartition, PartitionUpload},
    error::{AppError, Result},
    image::ImageKind,
    task::TaskState,
};

use crate::{AgentInfo, get_next_task};

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo)]
pub async fn upload_partition_data(
    agent: AgentInfo,
    Path((task_id, partition_number)): Path<(i64, i64)>,
    Query(PartitionUpload {
        filesystem,
        size,
        fs_uuid,
        fs_label,
    }): Query<PartitionUpload>,
    body: Body,
) -> Result<Json<ImagePartition>> {
    let (task, image_id) = running_capture(&agent.mac, task_id, TaskType::captures).await?;
    if task.task_type == TaskType::CapturePartition {
        if partition_number != 1 {
            return Err(AppError::InvalidArgument(format!(
                "a partition capture stores only partition 1, not {partition_number}"
            )));
        }
        let fstypes = ImageKind::Partition.fstypes();
        if !fstypes.contains(&filesystem.as_str()) {
            return Err(AppError::InvalidArgument(format!(
                "a partition image cannot hold a `{filesystem}` filesystem, expected one of {}",
                fstypes.join(", ")
            )));
        }
    }
    let stream = body
        .into_data_stream()
        .map_err(|e| AppError::Internal(e.to_string()));

    image_service
        .save_partition_data(image_id, partition_number, stream)
        .await?;

    let partition = image_repo
        .save_partition(
            image_id,
            partition_number,
            &filesystem,
            size,
            fs_uuid.as_deref(),
            fs_label.as_deref(),
        )
        .await?;

    Ok(Json(partition))
}

#[injectable::inject(image_service: ImageSvc)]
pub async fn partition_table(agent: AgentInfo, Path(task_id): Path<i64>, body: Bytes) -> Result {
    let (_, image_id) = running_capture(&agent.mac, task_id, TaskType::is_capture).await?;
    image_service.save_partition_table(image_id, &body).await
}

#[injectable::inject(image_repo: ImageRepo)]
pub async fn root_partition(
    agent: AgentInfo,
    Path((task_id, partition_number)): Path<(i64, i64)>,
) -> Result {
    let (_, image_id) = running_capture(&agent.mac, task_id, TaskType::captures).await?;
    image_repo
        .set_root_partition(image_id, Some(partition_number))
        .await?;
    Ok(())
}

async fn running_capture(
    mac: &str,
    task_id: i64,
    accepts: fn(&TaskType) -> bool,
) -> Result<(Task, i64)> {
    let task = get_next_task(mac).await?;
    let Some(image_id) = task.image_id else {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} is not valid"
        )));
    };
    if task.id != task_id || !accepts(&task.task_type) {
        return Err(AppError::InvalidArgument(format!(
            "No capture task for task {task_id} found"
        )));
    }
    if task.aggregate_state() != TaskState::Running {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ));
    }
    Ok((task, image_id))
}
