use crate::model;
use bytes::Bytes;
use dioxus_fullstack::*;
use imaged_shared::error::Result;

#[cfg(feature = "server")]
use crate::{AgentInfo, get_next_task};
#[cfg(feature = "server")]
use imaged_core::{
    di::{HostRepo, ImageRepo, ImageSvc, TaskRepo},
    domain::{
        image::ImageStatus,
        task::{Task, TaskType},
    },
};
#[cfg(feature = "server")]
use imaged_shared::error::AppError;

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo)]
#[get(
    "/api/client/tasks/{task_id}/partitions/{partition_number}/data",
    agent: AgentInfo
)]
pub async fn download_partition_data(task_id: i64, partition_number: i64) -> Result<ByteStream> {
    let (_, image_id) = get_restore_task_and_verify(&agent.mac, task_id).await?;
    if image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    let stream = image_service
        .read_partition_data(image_id, partition_number)
        .await?;

    Ok(ByteStream::from(stream))
}

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo, host_repo: HostRepo, task_repo: TaskRepo)]
#[get(
    "/api/client/tasks/{task_id}/parttable",
    agent: AgentInfo
)]
pub async fn download_partition_table(task_id: i64) -> Result<Bytes> {
    let (task, image_id) = get_restore_task_and_verify(&agent.mac, task_id).await?;
    if image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    let host_id = host_repo.get_by_mac(&agent.mac).await?.id;
    task_repo.start(task.id, host_id).await?;
    let data = image_service.read_partition_table(image_id).await?;
    Ok(Bytes::from(data))
}

#[injectable::inject(image_repo: ImageRepo)]
#[get("/api/client/tasks/{task_id}/partitions", agent: AgentInfo)]
pub async fn download_partitions(task_id: i64) -> Result<Vec<model::ImagePartition>> {
    let (_, image_id) = get_restore_task_and_verify(&agent.mac, task_id).await?;
    if image_repo.get_status(image_id).await? != ImageStatus::Ready {
        return Err(AppError::FailedPrecondition(format!(
            "Image {image_id} is not ready"
        )));
    };
    let partitions = image_repo.get_partitions(image_id).await?;

    Ok(partitions.into_iter().map(Into::into).collect())
}

#[cfg(feature = "server")]
async fn get_restore_task_and_verify(mac: &str, task_id: i64) -> Result<(Task, i64)> {
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
