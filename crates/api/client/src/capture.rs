use crate::model;
use bytes::Bytes;
use dioxus_fullstack::*;
use imaged_shared::error::Result;

#[cfg(feature = "server")]
use crate::{AgentInfo, get_next_task};
#[cfg(feature = "server")]
use futures::TryStreamExt;
#[cfg(feature = "server")]
use imaged_core::{
    di::{HostRepo, ImageRepo, ImageSvc, TaskRepo},
    domain::task::{Task, TaskState, TaskType},
};
#[cfg(feature = "server")]
use imaged_shared::error::AppError;

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo)]
#[put(
    "/api/client/tasks/{task_id}/partitions/{partition_number}/data?filesystem&size",
    agent: AgentInfo
)]
pub async fn upload_partition_data(
    task_id: i64,
    partition_number: i64,
    filesystem: String,
    size: i64,
    body: ByteStream,
) -> Result<model::ImagePartition> {
    let (task, image_id) = get_capture_task_and_verify(&agent.mac, task_id).await?;
    // Capture is single-host, so the aggregate equals that host's state.
    if task.aggregate_state() != TaskState::Running {
        return Err(AppError::InvalidArgument(
            "Task has not yet started".to_string(),
        ))?;
    }
    let stream = Box::pin(
        body.into_inner()
            .map_err(|e| AppError::Internal(e.to_string())),
    );

    image_service
        .save_partition_data(image_id, partition_number, stream)
        .await?;

    let partition = image_repo
        .save_partition(image_id, partition_number, &filesystem, size)
        .await?;

    Ok(partition.into())
}

#[injectable::inject(image_service: ImageSvc, image_repo: ImageRepo, host_repo: HostRepo, task_repo: TaskRepo)]
#[put(
    "/api/client/tasks/{task_id}/parttable",
    agent: AgentInfo
)]
pub async fn partition_table(task_id: i64, body: Bytes) -> Result {
    let (task, image_id) = get_capture_task_and_verify(&agent.mac, task_id).await?;
    if task.aggregate_state() != TaskState::Pending {
        return Err(AppError::InvalidArgument(
            "Task has already stared".to_string(),
        ));
    }

    image_service.clear_image_data(image_id).await?;
    // remove old partition records and marks image as capturing
    image_repo.start_capture(image_id).await?;
    // save partition table
    image_service.save_partition_table(image_id, &body).await?;
    // mark this host's row running
    let host_id = host_repo.get_by_mac(&agent.mac).await?.id;
    task_repo.start(task.id, host_id).await?;

    Ok(())
}

#[cfg(feature = "server")]
async fn get_capture_task_and_verify(
    mac: &str,
    task_id: i64,
) -> imaged_core::error::Result<(Task, i64)> {
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
