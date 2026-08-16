mod capture;
mod deploy;
mod sse;
mod task;

use std::sync::Arc;

use axum::{
    Router,
    extract::FromRequestParts,
    http::request::Parts,
    routing::{get, post, put},
};

use crate::{api::DIContainer, domain::task::Task, error::AppError};

pub fn router() -> Router<Arc<DIContainer>> {
    Router::new()
        .route(
            "/client/tasks/{task_id}/partitions/{partition_number}/data",
            put(capture::upload_partition_data).get(deploy::download_partition_data),
        )
        .route(
            "/client/tasks/{task_id}/partitions",
            get(deploy::download_partitions),
        )
        .route(
            "/client/tasks/{task_id}/parttable",
            put(capture::upload_partition_table).get(deploy::download_partition_table),
        )
        .route(
            "/client/tasks/{task_id}/finished",
            post(task::mark_finished),
        )
        .route("/client/tasks/{task_id}/faulted", post(task::mark_faulted))
        .route("/client/hosts/stream", get(sse::start_stream))
        .route("/client/hosts/disconnect", post(sse::disconnect))
}

pub struct AgentInfo(pub (String, Option<String>));

impl<S> FromRequestParts<S> for AgentInfo
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let mac = parts
            .headers
            .get("X-Agent-Mac")
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::InvalidArgument("missing mac".into()))?
            .to_string();

        let ip = parts
            .headers
            .get("X-Agent-Ip")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        Ok(AgentInfo((mac, ip)))
    }
}

async fn get_next_task(state: Arc<DIContainer>, mac: &str) -> crate::error::Result<Task> {
    let host = state.host_repo.get_by_mac(mac).await?;
    let task =
        state.task_repo.get_next(host.id).await?.ok_or_else(|| {
            AppError::InvalidArgument(format!("No active task found for host {mac}"))
        })?;
    Ok(task)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

    use crate::domain::task::TaskType;

    #[tokio::test]
    async fn partitions_endpoint_serves_the_image_filesystem_types() {
        let dir = std::env::temp_dir().join(format!("imaged-client-api-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = crate::setup_database(&format!("sqlite://{}", dir.join("test.db").display()))
            .await
            .unwrap();
        let state = crate::build_di_container(
            pool,
            dir.join("images").to_string_lossy().to_string(),
            "lo".to_string(),
            "127.0.0.1:8080".parse().unwrap(),
        )
        .await
        .unwrap();

        let host = state
            .host_repo
            .upsert_host("aa:bb:cc:dd:ee:01".into(), 1_000_000, None)
            .await
            .unwrap();
        let image = state.image_repo.create_image("img".into()).await.unwrap();
        state
            .image_repo
            .save_partition(image.id, 1, "vfat", 1_000)
            .await
            .unwrap();
        state
            .image_repo
            .save_partition(image.id, 2, "ext4", 2_000)
            .await
            .unwrap();
        state.image_repo.mark_finished(image.id).await.unwrap();
        let task = state
            .task_repo
            .create(TaskType::Multicast, vec![host.id], Some(image.id))
            .await
            .unwrap();

        let response = super::router()
            .with_state(std::sync::Arc::new(state))
            .oneshot(
                Request::builder()
                    .uri(format!("/client/tasks/{}/partitions", task.id))
                    .header("X-Agent-Mac", "aa:bb:cc:dd:ee:01")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let partitions: Vec<imaged_shared::ImagePartition> = serde_json::from_slice(&body).unwrap();

        let listed: Vec<(i64, &str)> = partitions
            .iter()
            .map(|p| (p.partition_number, p.fstype.as_str()))
            .collect();
        assert_eq!(listed, vec![(1, "vfat"), (2, "ext4")]);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
