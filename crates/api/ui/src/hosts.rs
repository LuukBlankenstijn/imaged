use dioxus::prelude::*;

use crate::model::{DeployRequest, Host, Task, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::{HostRepo, ImageRepo, MachineIp, Registry, ScriptRepo, TaskRepo};
#[cfg(feature = "server")]
use imaged_core::domain::script::{DeployInputs, InputValues, resolve_inputs};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
#[cfg(feature = "server")]
use imaged_core::error::AppError;
use imaged_model::error::Result;
#[cfg(feature = "server")]
use std::collections::BTreeMap;

#[get("/api/ui/hosts")]
#[inject(host_repo: HostRepo)]
pub async fn get_all_hosts() -> Result<Vec<Host>> {
    let hosts = host_repo.get_all(None).await?;
    Ok(hosts.into_iter().map(Into::into).collect())
}

#[get("/api/ui/groups/{group_id}/hosts")]
#[inject(host_repo: HostRepo)]
pub async fn get_hosts_by_group(group_id: i64) -> Result<Vec<Host>> {
    let hosts = host_repo.get_all(Some(group_id)).await?;
    Ok(hosts.into_iter().map(Into::into).collect())
}

#[post("/api/ui/hosts/rename")]
#[inject(host_repo: HostRepo)]
pub async fn update_host_name(req: UpdateName) -> Result<Host> {
    let host = host_repo.update_name(req.id, req.new_name).await?;
    Ok(host.into())
}

#[post("/api/ui/hosts/delete")]
#[inject(task_repo: TaskRepo, host_repo: HostRepo)]
pub async fn delete_host(id: i64) -> Result<()> {
    if task_repo.get_next(id).await?.is_some() {
        return Err(AppError::InvalidArgument(format!(
            "Host with id {id} has active tasks, finish or cancel those first"
        )));
    }
    host_repo.delete(id).await?;
    Ok(())
}

#[post("/api/ui/hosts/deploy")]
#[inject(task_repo: TaskRepo, registry: Registry)]
pub async fn deploy(req: DeployRequest) -> Result<Task> {
    let inputs = host_inputs(req.image_id, &[req.id], req.inputs.into()).await?;
    let task = task_repo
        .create_with_inputs(TaskType::Deploy, vec![req.id], Some(req.image_id), &inputs)
        .await?;
    registry.send_task(req.id, &task);
    Ok(task.into())
}

#[post("/api/ui/hosts/reboot")]
#[inject(task_repo: TaskRepo, registry: Registry)]
pub async fn reboot(host_ids: Vec<i64>) -> Result<()> {
    let task = task_repo
        .create(TaskType::Reboot, host_ids.clone(), None)
        .await?;
    for id in host_ids {
        registry.send_task(id, &task);
    }
    Ok(())
}

#[post("/api/ui/hosts/wake")]
#[inject(host_repo: HostRepo, machine_ip: MachineIp)]
pub async fn wake_on_lan(host_ids: Vec<i64>) -> Result<()> {
    let ip = machine_ip.read().ok_or_else(|| {
        AppError::FailedPrecondition("the machine listener is not running".into())
    })?;
    let src = std::net::SocketAddr::new(ip, 0);
    let dest = std::net::SocketAddr::from(([255, 255, 255, 255], 9));

    let wanted: std::collections::HashSet<i64> = host_ids.into_iter().collect();
    for host in host_repo
        .get_all(None)
        .await?
        .into_iter()
        .filter(|h| wanted.contains(&h.id))
    {
        let normalized = host.mac_address.replace('-', ":");
        match wakey::WolPacket::from_string(&normalized, ':') {
            Ok(packet) => {
                if let Err(e) = packet.send_magic_to(src, dest) {
                    tracing::warn!(mac = %host.mac_address, err = %e, "failed to send wake-on-lan packet");
                }
            }
            Err(e) => {
                tracing::warn!(mac = %host.mac_address, err = %e, "invalid mac address for wake-on-lan")
            }
        }
    }
    Ok(())
}

#[cfg(feature = "server")]
#[inject(image_repo: ImageRepo, script_repo: ScriptRepo)]
pub(crate) async fn host_inputs(
    image_id: i64,
    host_ids: &[i64],
    inputs: DeployInputs,
) -> Result<BTreeMap<i64, InputValues>> {
    let image = image_repo.get(image_id).await?;
    let scripts = script_repo.get_for_image(image_id).await?;
    if !scripts.is_empty() && image.root_partition.is_none() {
        return Err(AppError::FailedPrecondition(format!(
            "image {} has post-install scripts but no root partition",
            image.name
        )));
    }
    resolve_inputs(&scripts, host_ids, &inputs)
}
