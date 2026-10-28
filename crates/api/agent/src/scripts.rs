use std::collections::BTreeMap;

use axum::{
    Json,
    body::Body,
    extract::{Path, Query},
};
use futures::TryStreamExt;
use imaged_core::{
    di::{HostRepo, ImageRepo, ScriptProxy, ScriptRepo, ScriptVariables, TaskRepo},
    domain::{
        image::Image,
        script::{Script, script_roots},
        task::TaskType,
    },
    service::script_output::{ScriptRunKey, record_output},
};
use imaged_model::{
    agent::{PostInstall, PostInstallScript, ScriptFinished, ScriptOutputUpload},
    error::{AppError, Result},
    script::{InputValues, validate_script_name},
    task::TaskState,
};

use crate::{AgentInfo, deploy::get_restore_task_and_verify};

#[injectable::inject(image_repo: ImageRepo, script_proxy: ScriptProxy, script_variables: ScriptVariables)]
pub async fn post_install(agent: AgentInfo, Path(task_id): Path<i64>) -> Result<Json<PostInstall>> {
    let run = running_restore_task(&agent.mac, task_id).await?;
    let image = image_repo.get(run.image_id).await?;
    Ok(Json(PostInstall {
        proxy: script_proxy.as_deref().map(str::to_owned),
        variables: BTreeMap::clone(&script_variables),
        scripts: planned_scripts(task_id, run.host_id, &image).await?,
    }))
}

#[injectable::inject(script_repo: ScriptRepo, task_repo: TaskRepo)]
pub(crate) async fn planned_scripts(
    task_id: i64,
    host_id: i64,
    image: &Image,
) -> Result<Vec<PostInstallScript>> {
    let scripts = script_repo.get_for_image(image.id).await?;
    let roots = script_roots(image, &scripts)?;
    let values = task_repo.get_inputs(task_id, host_id).await?;
    scripts
        .into_iter()
        .zip(roots)
        .zip(0..)
        .map(|((attached, root_partition), position)| {
            let script = attached.script;
            Ok(PostInstallScript {
                position,
                inputs: script_inputs(&script, &values)?,
                name: script.name,
                body: script.body,
                root_partition,
            })
        })
        .collect()
}

fn script_inputs(script: &Script, values: &InputValues) -> Result<BTreeMap<String, String>> {
    let stale = |reason: String| {
        AppError::FailedPrecondition(format!(
            "script `{}` no longer fits the inputs stored with this task: {reason}; start a new deployment",
            script.name
        ))
    };
    let mut inputs = BTreeMap::new();
    for input in &script.inputs {
        match input.value_from(values) {
            Some(value) => {
                input.check(value).map_err(stale)?;
                inputs.insert(input.name.clone(), value.to_owned());
            }
            None if input.required => {
                return Err(stale(format!("input `{}` is required", input.name)));
            }
            None => {}
        }
    }
    Ok(inputs)
}

#[injectable::inject(task_repo: TaskRepo)]
pub async fn upload_script_output(
    agent: AgentInfo,
    Path((task_id, position)): Path<(i64, i64)>,
    Query(ScriptOutputUpload { name }): Query<ScriptOutputUpload>,
    body: Body,
) -> Result {
    validate_script_name(&name)?;
    if position < 0 {
        return Err(AppError::InvalidArgument(format!(
            "script position {position} is negative"
        )));
    }
    let run = running_restore_task(&agent.mac, task_id).await?;
    task_repo
        .start_script_run(task_id, run.host_id, position, &name)
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
    let (task, image_id) = get_restore_task_and_verify(mac, task_id, TaskType::restores).await?;
    let host_id = host_repo.get_by_mac(mac).await?.id;
    if !task.hosts_in(TaskState::Running).any(|id| id == host_id) {
        return Err(AppError::InvalidArgument(format!(
            "Task {task_id} has not started on this host"
        )));
    }
    Ok(RestoreRun { image_id, host_id })
}
