use dioxus::fullstack::FileStream;
use dioxus::prelude::*;

use crate::model::{CreateImageRequest, Image, ImageCapture, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use dioxus::fullstack::body::Body;
#[cfg(feature = "server")]
use futures::TryStreamExt;
#[cfg(feature = "server")]
use imaged_archive::archive_file_name;
#[cfg(feature = "server")]
use imaged_core::di::{ArchiveSvc, ImageRepo, ImageSvc, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
#[cfg(feature = "server")]
use imaged_model::error::AppError;
use imaged_model::error::Result;
#[cfg(feature = "server")]
use tokio_util::io::StreamReader;

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
pub async fn create_image(req: CreateImageRequest) -> Result<ImageCapture> {
    crate::hosts::imaging_targets(&[req.host_id]).await?;
    let image = image_repo.create_image(req.name).await?;
    let task = task_repo
        .create(TaskType::Capture, vec![req.host_id], Some(image.id))
        .await?;
    registry.dispatch(req.host_id);
    Ok(ImageCapture {
        image: image.into(),
        task: task.into(),
    })
}

#[post("/api/ui/images/delete")]
#[inject(task_repo: TaskRepo, image_repo: ImageRepo, image_svc: ImageSvc)]
pub async fn delete_image(id: i64) -> Result<()> {
    let active = task_repo.get_active_by_image(id).await?;
    let restores: Vec<String> = active
        .iter()
        .filter(|task| task.task_type != TaskType::Capture)
        .map(|task| format!("#{}", task.id))
        .collect();
    if !restores.is_empty() {
        let image = image_repo.get(id).await?;
        return Err(AppError::FailedPrecondition(format!(
            "image {} is being deployed by task(s) {}, finish or cancel them first",
            image.name,
            restores.join(", ")
        )));
    }
    for task in &active {
        crate::tasks::cancel_active(task).await?;
    }
    image_repo.delete_image(id).await?;
    image_svc.clear_image_data(id).await?;
    Ok(())
}

#[get("/api/ui/images/{id}/export")]
#[inject(archive_svc: ArchiveSvc)]
pub async fn export_image(id: i64) -> Result<FileStream> {
    let image = archive_svc.exportable(id).await?;
    let archive = Body::from_stream(archive_svc.export_stream(id)).into_data_stream();
    Ok(FileStream::from_raw(
        archive_file_name(&image.name),
        None,
        "application/x-tar".to_string(),
        archive,
    ))
}

#[post("/api/ui/images/import")]
#[inject(archive_svc: ArchiveSvc)]
pub async fn import_image(archive: FileStream) -> Result<Image> {
    let reader = StreamReader::new(archive.map_err(std::io::Error::other));
    let image = archive_svc.import(reader).await?;
    Ok(image.into())
}
