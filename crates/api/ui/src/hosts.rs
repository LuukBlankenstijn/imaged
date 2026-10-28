use dioxus::prelude::*;

use crate::model::{DeployRequest, Host, Task, UpdateName};

#[cfg(feature = "server")]
use injectable::inject;

#[cfg(feature = "server")]
use imaged_core::di::{
    HostRepo, ImageRepo, MachineIp, Registry, ScriptRepo, ScriptVariables, TaskRepo,
};
#[cfg(feature = "server")]
use imaged_core::domain::host::Host as DHost;
#[cfg(feature = "server")]
use imaged_core::domain::image::Image as DImage;
#[cfg(feature = "server")]
use imaged_core::domain::script::{Script as DScript, resolve_inputs, script_roots};
#[cfg(feature = "server")]
use imaged_core::domain::task::TaskType;
#[cfg(feature = "server")]
use imaged_model::error::AppError;
use imaged_model::error::Result;
#[cfg(feature = "server")]
use imaged_model::host::HostPartition;
#[cfg(feature = "server")]
use imaged_model::image::ImageKind;
#[cfg(feature = "server")]
use imaged_model::script::DeployInputs;
#[cfg(feature = "server")]
use imaged_model::script::InputValues;
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
    let partitions = req.partition.map(|p| (req.id, p)).into_iter().collect();
    let plan = plan_restore(req.image_id, &[req.id], req.inputs, partitions).await?;
    let task = task_repo
        .create_with_inputs(
            TaskType::deploy(plan.kind),
            vec![req.id],
            Some(req.image_id),
            &plan.inputs,
            &plan.partuuids,
        )
        .await?;
    registry.dispatch(req.id);
    Ok(task.into())
}

#[post("/api/ui/hosts/reboot")]
#[inject(task_repo: TaskRepo, registry: Registry)]
pub async fn reboot(host_ids: Vec<i64>) -> Result<Task> {
    let task = task_repo
        .create(TaskType::Reboot, host_ids.clone(), None)
        .await?;
    for id in host_ids {
        registry.dispatch(id);
    }
    Ok(task.into())
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
        match wakey::WolPacket::from_string(&host.mac_address, ':') {
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
#[inject(host_repo: HostRepo)]
pub(crate) async fn imaging_targets(host_ids: &[i64]) -> Result<Vec<DHost>> {
    let mut known: BTreeMap<i64, DHost> = host_repo
        .get_all(None)
        .await?
        .into_iter()
        .map(|h| (h.id, h))
        .collect();
    let hosts = host_ids
        .iter()
        .map(|id| {
            known
                .remove(id)
                .ok_or_else(|| AppError::NotFound(format!("host {id}")))
        })
        .collect::<Result<Vec<_>>>()?;
    for host in &hosts {
        host.usable_disk()?;
    }
    Ok(hosts)
}

#[cfg(feature = "server")]
pub(crate) struct RestorePlan {
    pub kind: ImageKind,
    pub inputs: BTreeMap<i64, InputValues>,
    pub partuuids: BTreeMap<i64, String>,
}

#[cfg(feature = "server")]
#[inject(image_repo: ImageRepo, script_repo: ScriptRepo, script_variables: ScriptVariables)]
pub(crate) async fn plan_restore(
    image_id: i64,
    host_ids: &[i64],
    inputs: DeployInputs,
    partitions: BTreeMap<i64, String>,
) -> Result<RestorePlan> {
    let image = image_repo.get(image_id).await?;
    if !image.status.is_ready() {
        return Err(AppError::FailedPrecondition(format!(
            "image {} is {}, only a ready image can be deployed",
            image.name, image.status
        )));
    }
    let hosts = imaging_targets(host_ids).await?;
    let partuuids = target_partitions(&image, &hosts, partitions)?;
    let scripts = script_repo.get_for_image(image_id).await?;
    script_roots(&image, &scripts)?;
    let scripts: Vec<DScript> = scripts
        .into_iter()
        .map(|attached| attached.script)
        .collect();
    Ok(RestorePlan {
        kind: image.kind,
        inputs: resolve_inputs(&scripts, &hosts, &inputs.resolve(&script_variables)?)?,
        partuuids,
    })
}

#[cfg(feature = "server")]
fn target_partitions(
    image: &DImage,
    hosts: &[DHost],
    mut partitions: BTreeMap<i64, String>,
) -> Result<BTreeMap<i64, String>> {
    if image.kind.is_disk() {
        if !partitions.is_empty() {
            return Err(AppError::InvalidArgument(format!(
                "image {} is a disk image, it replaces the whole disk and cannot be deployed into a partition",
                image.name
            )));
        }
        return Ok(partitions);
    }
    if let Some(stray) = partitions
        .keys()
        .find(|id| !hosts.iter().any(|host| host.id == **id))
    {
        return Err(AppError::InvalidArgument(format!(
            "host {stray} is not part of this deployment"
        )));
    }
    let [source] = image.partitions.as_slice() else {
        return Err(AppError::FailedPrecondition(format!(
            "partition image {} does not hold exactly one partition",
            image.name
        )));
    };
    let mut targets = BTreeMap::new();
    for host in hosts {
        let partuuid = partitions.remove(&host.id).ok_or_else(|| {
            AppError::InvalidArgument(format!(
                "image {} is a partition image, pick the partition it replaces on host {}",
                image.name, host.name
            ))
        })?;
        let target = host.partition(&partuuid)?;
        if target.size_bytes < source.size_bytes {
            return Err(AppError::FailedPrecondition(format!(
                "partition {} of host {} holds {} bytes, image {} needs at least {}",
                target.partuuid, host.name, target.size_bytes, image.name, source.size_bytes
            )));
        }
        let twin = |holds: &dyn Fn(&HostPartition) -> bool| {
            host.partitions
                .iter()
                .find(|other| other.number != target.number && holds(other))
        };
        if let Some(fs_uuid) = &source.fs_uuid
            && let Some(twin) = twin(&|other| {
                other
                    .uuid
                    .as_deref()
                    .is_some_and(|uuid| uuid.eq_ignore_ascii_case(fs_uuid))
            })
        {
            return Err(AppError::FailedPrecondition(format!(
                "partition {} of host {} already holds filesystem {fs_uuid} of image {}, a second copy would make the filesystem UUID ambiguous",
                twin.partuuid, host.name, image.name
            )));
        }
        if let Some(fs_label) = &source.fs_label
            && let Some(twin) = twin(&|other| other.label.as_ref() == Some(fs_label))
        {
            return Err(AppError::FailedPrecondition(format!(
                "partition {} of host {} already holds a filesystem labelled `{fs_label}` like image {}, a second copy would make the filesystem label ambiguous",
                twin.partuuid, host.name, image.name
            )));
        }
        targets.insert(host.id, target.partuuid.clone());
    }
    Ok(targets)
}
