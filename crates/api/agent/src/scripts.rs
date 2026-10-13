use std::collections::BTreeMap;

use axum::{Json, body::Body, extract::Path};
use futures::TryStreamExt;
use imaged_core::{
    di::{HostRepo, ImageRepo, ScriptProxy, ScriptRepo, ScriptVariables, TaskRepo},
    domain::script::Script,
    service::script_output::{ScriptRunKey, record_output},
};
use imaged_model::{
    agent::{PostInstall, PostInstallScript, ScriptFinished},
    error::{AppError, Result},
    task::TaskState,
};

use crate::{AgentInfo, deploy::get_restore_task_and_verify};

#[injectable::inject(image_repo: ImageRepo, script_repo: ScriptRepo, task_repo: TaskRepo, script_proxy: ScriptProxy, script_variables: ScriptVariables)]
pub async fn post_install(agent: AgentInfo, Path(task_id): Path<i64>) -> Result<Json<PostInstall>> {
    let run = running_restore_task(&agent.mac, task_id).await?;
    let image = image_repo.get(run.image_id).await?;
    let scripts = script_repo.get_for_image(run.image_id).await?;
    if !scripts.is_empty() && image.root_partition.is_none() {
        return Err(AppError::FailedPrecondition(format!(
            "image {} has scripts but no root partition",
            image.id
        )));
    }
    let values = task_repo.get_inputs(task_id, run.host_id).await?;
    Ok(Json(PostInstall {
        root_partition: image.root_partition,
        proxy: script_proxy.as_deref().map(str::to_owned),
        variables: BTreeMap::clone(&script_variables),
        scripts: scripts
            .into_iter()
            .zip(0..)
            .map(|(script, position)| PostInstallScript {
                position,
                inputs: script
                    .inputs
                    .iter()
                    .filter_map(|input| {
                        let value = input.value_from(&values)?;
                        Some((input.name.clone(), value.to_owned()))
                    })
                    .collect(),
                name: script.name,
                body: script.body,
            })
            .collect(),
    }))
}

#[injectable::inject(script_repo: ScriptRepo, task_repo: TaskRepo)]
pub async fn upload_script_output(
    agent: AgentInfo,
    Path((task_id, position)): Path<(i64, i64)>,
    body: Body,
) -> Result {
    let run = running_restore_task(&agent.mac, task_id).await?;
    let script = script_at(script_repo.get_for_image(run.image_id).await?, position)?;
    task_repo
        .start_script_run(task_id, run.host_id, position, &script.name)
        .await?;
    let output = body
        .into_data_stream()
        .map_err(|e| AppError::Internal(e.to_string()));
    record_output(
        &**task_repo,
        ScriptRunKey {
            task_id,
            host_id: run.host_id,
            position,
        },
        output,
    )
    .await
}

#[injectable::inject(task_repo: TaskRepo)]
pub async fn finish_script(
    agent: AgentInfo,
    Path((task_id, position)): Path<(i64, i64)>,
    Json(ScriptFinished { exit_code }): Json<ScriptFinished>,
) -> Result {
    let run = running_restore_task(&agent.mac, task_id).await?;
    task_repo
        .finish_script_run(task_id, run.host_id, position, exit_code)
        .await
}

struct RestoreRun {
    image_id: i64,
    host_id: i64,
}

#[injectable::inject(host_repo: HostRepo)]
async fn running_restore_task(mac: &str, task_id: i64) -> Result<RestoreRun> {
    let (task, image_id) = get_restore_task_and_verify(mac, task_id).await?;
    let host_id = host_repo.get_by_mac(mac).await?.id;
    if !task.hosts_in(TaskState::Running).any(|id| id == host_id) {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} has not started on this host"
        )));
    }
    Ok(RestoreRun { image_id, host_id })
}

fn script_at(scripts: Vec<Script>, position: i64) -> Result<Script> {
    usize::try_from(position)
        .ok()
        .and_then(|index| scripts.into_iter().nth(index))
        .ok_or_else(|| AppError::InvalidArgument(format!("the image has no script {position}")))
}
