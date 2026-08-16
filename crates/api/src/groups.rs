use dioxus::prelude::*;

use crate::model::{CreateGroupRequest, Group, MulticastRequest, UpdateGroupRequest, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use crate::error::sfe;
#[cfg(feature = "server")]
use imaged_core::di::{GroupRepo, MulticastMgr, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;

#[post("/api/ui/groups/create")]
#[inject(group_repo: GroupRepo)]
pub async fn create_group(req: CreateGroupRequest) -> ServerFnResult<Group> {
    let group = group_repo
        .create_group(&req.name, &req.host_ids)
        .await
        .map_err(sfe)?;
    Ok(group.into())
}

#[post("/api/ui/groups/rename")]
#[inject(group_repo: GroupRepo)]
pub async fn update_group_name(req: UpdateName) -> ServerFnResult<Group> {
    let group = group_repo
        .update_name(req.id, &req.new_name)
        .await
        .map_err(sfe)?;
    Ok(group.into())
}

#[get("/api/ui/groups")]
#[inject(group_repo: GroupRepo)]
pub async fn get_all_groups() -> ServerFnResult<Vec<Group>> {
    let groups = group_repo.get_all().await.map_err(sfe)?;
    Ok(groups.into_iter().map(Into::into).collect())
}

#[post("/api/ui/groups/members")]
#[inject(group_repo: GroupRepo)]
pub async fn update_group_memberships(req: UpdateGroupRequest) -> ServerFnResult<Group> {
    let group = group_repo
        .update_group_members(req.id, &req.host_ids)
        .await
        .map_err(sfe)?;
    Ok(group.into())
}

#[post("/api/ui/groups/delete")]
#[inject(group_repo: GroupRepo)]
pub async fn delete_group(id: i64) -> ServerFnResult<()> {
    group_repo.delete(id).await.map_err(sfe)?;
    Ok(())
}

#[post("/api/ui/groups/multicast")]
#[inject(task_repo: TaskRepo, multicast_mgr: MulticastMgr, registry: Registry)]
pub async fn multicast(req: MulticastRequest) -> ServerFnResult<()> {
    let task = task_repo
        .create(TaskType::Multicast, req.host_ids.clone(), Some(req.image_id))
        .await
        .map_err(sfe)?;
    multicast_mgr.notify_new(task.id).map_err(sfe)?;
    for id in req.host_ids {
        registry.send_task(id, &task);
    }
    Ok(())
}
