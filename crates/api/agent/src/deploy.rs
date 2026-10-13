use axum::{Json, body::Body, extract::Path};
use imaged_core::{
    di::{HostRepo, ImageRepo, ImageSvc, TaskRepo},
    domain::{
        image::ImageStatus,
        task::{Task, TaskType},
    },
};
use imaged_model::{
    agent::ImagePartition,
    error::{AppError, Result},
};

use crate::{AgentInfo, get_next_task};

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo)]
pub async fn download_partition_data(
    agent: AgentInfo,
    Path((task_id, partition_number)): Path<(i64, i64)>,
) -> Result<Body> {
    let (_, image_id) = get_restore_task_and_verify(&agent.mac, task_id).await?;
    if image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    let stream = image_service
        .read_partition_data(image_id, partition_number)
        .await?;

    Ok(Body::from_stream(stream))
}

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo, host_repo: HostRepo, task_repo: TaskRepo)]
pub async fn download_partition_table(
    agent: AgentInfo,
    Path(task_id): Path<i64>,
) -> Result<Vec<u8>> {
    let (task, image_id) = get_restore_task_and_verify(&agent.mac, task_id).await?;
    if image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    let host_id = host_repo.get_by_mac(&agent.mac).await?.id;
    task_repo.start(task.id, host_id).await?;
    image_service.read_partition_table(image_id).await
}

#[injectable::inject(image_repo: ImageRepo)]
pub async fn download_partitions(
    agent: AgentInfo,
    Path(task_id): Path<i64>,
) -> Result<Json<Vec<ImagePartition>>> {
    let (_, image_id) = get_restore_task_and_verify(&agent.mac, task_id).await?;
    if image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    let partitions = image_repo.get_partitions(image_id).await?;

    Ok(Json(partitions.into_iter().map(Into::into).collect()))
}

pub(crate) async fn get_restore_task_and_verify(mac: &str, task_id: i64) -> Result<(Task, i64)> {
    let task = get_next_task(mac).await?;
    if task.id != task_id || !matches!(task.task_type, TaskType::Deploy | TaskType::Multicast) {
        return Err(AppError::InvalidArgument(format!(
            "No restore task for task {task_id} found"
        )));
    }

    let Some(image_id) = task.image_id else {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} has no image"
        )));
    };

    Ok((task, image_id))
}
