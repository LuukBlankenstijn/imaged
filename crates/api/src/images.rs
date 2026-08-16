use dioxus::prelude::*;

use crate::model::{CreateImageRequest, Image, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use crate::error::sfe;
#[cfg(feature = "server")]
use imaged_core::di::{ImageRepo, ImageSvc, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
#[cfg(feature = "server")]
use imaged_core::error::AppError;

#[get("/api/ui/images")]
#[inject(image_repo: ImageRepo)]
pub async fn get_all_images() -> ServerFnResult<Vec<Image>> {
    let images = image_repo.get_all().await.map_err(sfe)?;
    Ok(images.into_iter().map(Into::into).collect())
}

#[post("/api/ui/images/rename")]
#[inject(image_repo: ImageRepo)]
pub async fn update_image_name(req: UpdateName) -> ServerFnResult<Image> {
    let image = image_repo
        .update_name(req.id, req.new_name)
        .await
        .map_err(sfe)?;
    Ok(image.into())
}

#[post("/api/ui/images/create")]
#[inject(image_repo: ImageRepo, task_repo: TaskRepo, registry: Registry)]
pub async fn create_image(req: CreateImageRequest) -> ServerFnResult<Image> {
    let image = image_repo.create_image(req.name).await.map_err(sfe)?;
    let task = task_repo
        .create(TaskType::Capture, vec![req.host_id], Some(image.id))
        .await
        .map_err(sfe)?;
    registry.send_task(req.host_id, &task);
    Ok(image.into())
}

#[post("/api/ui/images/delete")]
#[inject(task_repo: TaskRepo, image_repo: ImageRepo, image_svc: ImageSvc)]
pub async fn delete_image(id: i64) -> ServerFnResult<()> {
    if !task_repo
        .get_active_by_image(id)
        .await
        .map_err(sfe)?
        .is_empty()
    {
        return Err(sfe(AppError::InvalidArgument(format!(
            "image with id {id} has active tasks, first cancel or complete those"
        ))));
    }
    image_repo.delete_image(id).await.map_err(sfe)?;
    image_svc.clear_image_data(id).await.map_err(sfe)?;
    Ok(())
}
