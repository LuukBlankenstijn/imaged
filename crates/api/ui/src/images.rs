use dioxus::fullstack::FileStream;
use dioxus::prelude::*;

use crate::model::{CreateImageRequest, Image, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use dioxus::fullstack::body::Body;
#[cfg(feature = "server")]
use futures::TryStreamExt;
#[cfg(feature = "server")]
use imaged_core::di::{ArchiveSvc, ImageRepo, ImageSvc, MulticastMgr, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
#[cfg(feature = "server")]
use imaged_core::service::archive::archive_file_name;
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

#[cfg(feature = "server")]
async fn cancel_task_effects(
    task: &imaged_core::domain::task::Task,
    task_repo: &TaskRepo,
    multicast_mgr: &MulticastMgr,
    registry: &Registry,
) -> Result<()> {
    task_repo.cancel(task.id).await?;
    if task.task_type == TaskType::Multicast {
        multicast_mgr.wake();
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
