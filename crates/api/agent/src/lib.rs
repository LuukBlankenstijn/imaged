mod agent;
mod capture;
mod deploy;
mod event;
mod scripts;
mod task;

use axum::{
    Router,
    routing::{get, post, put},
};
use imaged_core::{
    di::{HostRepo, TaskRepo},
    domain::task::Task,
};
use imaged_model::{
    agent::route,
    error::{AppError, Result},
};

use agent::AgentInfo;

pub fn router() -> Router {
    Router::new()
        .route(route::STREAM, get(event::start_stream))
        .route(route::DISCONNECT, post(event::disconnect))
        .route(route::STARTED, post(task::started))
        .route(route::FINISHED, post(task::mark_finished))
        .route(route::FAILED, post(task::mark_failed))
        .route(
            route::PARTITION_TABLE,
            put(capture::partition_table).get(deploy::download_partition_table),
        )
        .route(route::PARTITIONS, get(deploy::download_partitions))
        .route(
            route::PARTITION_DATA,
            put(capture::upload_partition_data).get(deploy::download_partition_data),
        )
        .route(route::ROOT_PARTITION, put(capture::root_partition))
        .route(route::SCRIPTS, get(scripts::post_install))
        .route(route::SCRIPT_OUTPUT, put(scripts::upload_script_output))
        .route(route::SCRIPT_FINISHED, post(scripts::finish_script))
}

#[injectable::inject(host_repo: HostRepo, task_repo: TaskRepo)]
async fn get_next_task(mac: &str) -> Result<Task> {
    let host = host_repo.get_by_mac(mac).await?;
    let task = task_repo
        .get_next(host.id)
        .await?
        .ok_or_else(|| AppError::InvalidArgument(format!("No active task found for host {mac}")))?;
    Ok(task)
}

#[cfg(test)]
mod tests;
