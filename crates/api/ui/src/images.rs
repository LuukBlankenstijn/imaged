use dioxus::prelude::*;

use crate::model::{CreateImageRequest, Image, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::{ImageRepo, ImageSvc, MulticastMgr, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
use imaged_shared::error::Result;

#[get("/api/ui/images")]
#[inject(image_repo: ImageRepo)]
pub async fn get_all_images() -> Result<Vec<Image>> {
    let images = image_repo.get_all().await?;
    Ok(images.into_iter().map(Into::into).collect())
}

#[post("/api/ui/images/rename")]
#[inject(image_repo: ImageRepo)]
pub async fn update_image_name(req: UpdateName) -> Result<Image> {
    let image = image_repo.update_name(req.id, req.new_name).await?;
    Ok(image.into())
}

#[post("/api/ui/images/create")]
#[inject(image_repo: ImageRepo, task_repo: TaskRepo, registry: Registry)]
pub async fn create_image(req: CreateImageRequest) -> Result<Image> {
    let image = image_repo.create_image(req.name).await?;
    let task = task_repo
        .create(TaskType::Capture, vec![req.host_id], Some(image.id))
        .await?;
    registry.send_task(req.host_id, &task);
    Ok(image.into())
}

#[post("/api/ui/images/delete")]
#[inject(task_repo: TaskRepo, image_repo: ImageRepo, image_svc: ImageSvc, multicast_mgr: MulticastMgr, registry: Registry)]
pub async fn delete_image(id: i64) -> Result<()> {
    for task in task_repo.get_active_by_image(id).await? {
        cancel_task_effects(&task, &task_repo, &multicast_mgr, &registry).await?;
    }
    image_repo.delete_image(id).await?;
    image_svc.clear_image_data(id).await?;
    Ok(())
}

#[cfg(feature = "server")]
async fn cancel_task_effects(
    task: &imaged_core::domain::task::Task,
    task_repo: &TaskRepo,
    multicast_mgr: &MulticastMgr,
    registry: &Registry,
) -> Result<()> {
    task_repo.cancel(task.id).await?;
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
