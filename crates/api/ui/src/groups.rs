use dioxus::prelude::*;

use crate::model::{
    CreateGroupRequest, Group, MulticastRequest, Task, UpdateGroupRequest, UpdateName,
};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::{GroupRepo, MulticastMgr, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
use imaged_model::error::Result;

#[post("/api/ui/groups/create")]
#[inject(group_repo: GroupRepo)]
pub async fn create_group(req: CreateGroupRequest) -> Result<Group> {
    group_repo.create_group(&req.name, &req.host_ids).await
}

#[post("/api/ui/groups/rename")]
#[inject(group_repo: GroupRepo)]
pub async fn update_group_name(req: UpdateName) -> Result<Group> {
    group_repo.update_name(req.id, &req.new_name).await
}

#[get("/api/ui/groups")]
#[inject(group_repo: GroupRepo)]
pub async fn get_all_groups() -> Result<Vec<Group>> {
    group_repo.get_all().await
}

#[post("/api/ui/groups/members")]
#[inject(group_repo: GroupRepo)]
pub async fn update_group_memberships(req: UpdateGroupRequest) -> Result<Group> {
    group_repo.update_group_members(req.id, &req.host_ids).await
}

#[post("/api/ui/groups/delete")]
#[inject(group_repo: GroupRepo)]
pub async fn delete_group(id: i64) -> Result<()> {
    group_repo.delete(id).await?;
    Ok(())
}

#[post("/api/ui/groups/multicast")]
#[inject(task_repo: TaskRepo, multicast_mgr: MulticastMgr)]
pub async fn multicast(req: MulticastRequest) -> Result<Task> {
    let plan =
        crate::hosts::plan_restore(req.image_id, &req.host_ids, req.inputs, req.partitions).await?;
    let task = task_repo
        .create_with_inputs(
            TaskType::multicast(plan.kind),
            req.host_ids,
            Some(req.image_id),
            &plan.inputs,
            &plan.partuuids,
        )
        .await?;
    multicast_mgr.wake();
    Ok(task.into())
}
