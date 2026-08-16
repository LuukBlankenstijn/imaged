use dioxus::prelude::*;

use crate::model::{DeployRequest, Host, Task, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use crate::error::sfe;
#[cfg(feature = "server")]
use imaged_core::api::send_wake_on_lan;
#[cfg(feature = "server")]
use imaged_core::di::{BindAddress, HostRepo, Registry, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
#[cfg(feature = "server")]
use imaged_core::error::AppError;

#[get("/api/ui/hosts")]
#[inject(host_repo: HostRepo)]
pub async fn get_all_hosts() -> ServerFnResult<Vec<Host>> {
    let hosts = host_repo.get_all(None).await.map_err(sfe)?;
    Ok(hosts.into_iter().map(Into::into).collect())
}

#[post("/api/ui/hosts/by-group")]
#[inject(host_repo: HostRepo)]
pub async fn get_hosts_by_group(group_id: i64) -> ServerFnResult<Vec<Host>> {
    let hosts = host_repo.get_all(Some(group_id)).await.map_err(sfe)?;
    Ok(hosts.into_iter().map(Into::into).collect())
}

#[post("/api/ui/hosts/rename")]
#[inject(host_repo: HostRepo)]
pub async fn update_host_name(req: UpdateName) -> ServerFnResult<Host> {
    let host = host_repo
        .update_name(req.id, req.new_name)
        .await
        .map_err(sfe)?;
    Ok(host.into())
}

#[post("/api/ui/hosts/delete")]
#[inject(task_repo: TaskRepo, host_repo: HostRepo)]
pub async fn delete_host(id: i64) -> ServerFnResult<()> {
    if task_repo.get_next(id).await.map_err(sfe)?.is_some() {
        return Err(sfe(AppError::InvalidArgument(format!(
            "Host with id {id} has active tasks, finish or cancel those first"
        ))));
    }
    host_repo.delete(id).await.map_err(sfe)?;
    Ok(())
}

#[post("/api/ui/hosts/deploy")]
#[inject(task_repo: TaskRepo, registry: Registry)]
pub async fn deploy(req: DeployRequest) -> ServerFnResult<Task> {
    let task = task_repo
        .create(TaskType::Deploy, vec![req.id], Some(req.image_id))
        .await
        .map_err(sfe)?;
    registry.send_task(req.id, &task);
    Ok(task.into())
}

#[post("/api/ui/hosts/reboot")]
#[inject(task_repo: TaskRepo, registry: Registry)]
pub async fn reboot(host_ids: Vec<i64>) -> ServerFnResult<()> {
    let task = task_repo
        .create(TaskType::Reboot, host_ids.clone(), None)
        .await
        .map_err(sfe)?;
    for id in host_ids {
        registry.send_task(id, &task);
    }
    Ok(())
}

#[post("/api/ui/hosts/wake")]
#[inject(host_repo: HostRepo, bind_address: BindAddress)]
pub async fn wake_on_lan(host_ids: Vec<i64>) -> ServerFnResult<()> {
    send_wake_on_lan(&host_repo, *bind_address, host_ids)
        .await
        .map_err(sfe)?;
    Ok(())
}
