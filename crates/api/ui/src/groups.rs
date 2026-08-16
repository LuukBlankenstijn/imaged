use dioxus::prelude::*;

use crate::model::{CreateGroupRequest, Group, MulticastRequest, UpdateGroupRequest, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::{GroupRepo, MulticastMgr, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
use imaged_shared::error::Result;

#[post("/api/ui/groups/create")]
#[inject(group_repo: GroupRepo)]
pub async fn create_group(req: CreateGroupRequest) -> Result<Group> {
    let group = group_repo.create_group(&req.name, &req.host_ids).await?;
    Ok(group.into())
}

#[post("/api/ui/groups/rename")]
#[inject(group_repo: GroupRepo)]
pub async fn update_group_name(req: UpdateName) -> Result<Group> {
    let group = group_repo.update_name(req.id, &req.new_name).await?;
    Ok(group.into())
}

#[get("/api/ui/groups")]
#[inject(group_repo: GroupRepo)]
pub async fn get_all_groups() -> Result<Vec<Group>> {
    let groups = group_repo.get_all().await?;
    Ok(groups.into_iter().map(Into::into).collect())
}

#[post("/api/ui/groups/members")]
#[inject(group_repo: GroupRepo)]
pub async fn update_group_memberships(req: UpdateGroupRequest) -> Result<Group> {
    let group = group_repo
        .update_group_members(req.id, &req.host_ids)
        .await?;
    Ok(group.into())
}

#[post("/api/ui/groups/delete")]
#[inject(group_repo: GroupRepo)]
pub async fn delete_group(id: i64) -> Result<()> {
    group_repo.delete(id).await?;
    Ok(())
}

#[post("/api/ui/groups/multicast")]
#[inject(task_repo: TaskRepo, multicast_mgr: MulticastMgr, registry: Registry)]
pub async fn multicast(req: MulticastRequest) -> Result<()> {
    let task = task_repo
        .create(
            TaskType::Multicast,
            req.host_ids.clone(),
            Some(req.image_id),
        )
        .await?;
    multicast_mgr.notify_new(task.id)?;
    for id in req.host_ids {
        registry.send_task(id, &task);
    }
    Ok(())
}
