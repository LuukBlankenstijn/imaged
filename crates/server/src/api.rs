use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use derive_more::Constructor;
use serde_json::json;

use crate::{
    domain::{
        group::GroupRepository, host::HostRepository, image::ImageRepository, task::TaskRepository,
    },
    error::AppError,
    multicast::MulticastManager,
    registry::HostRegistry,
    service::image::ImageService,
};

pub mod client;
pub mod pxe;

#[derive(Clone, Constructor)]
pub struct HandlerState {
    pub host_repo: Arc<dyn HostRepository>,
    pub host_registry: Arc<HostRegistry>,
    pub image_repo: Arc<dyn ImageRepository>,
    pub task_repo: Arc<dyn TaskRepository>,
    pub group_repo: Arc<dyn GroupRepository>,
    pub image_service: Arc<ImageService>,
    pub multicast_manager: Arc<MulticastManager>,
    pub bind_address: SocketAddr,
}

pub async fn send_wake_on_lan(
    host_repo: &Arc<dyn HostRepository>,
    bind_address: SocketAddr,
    host_ids: Vec<i64>,
) -> crate::error::Result<()> {
    let mut src = bind_address;
    src.set_port(0);
    let dest = SocketAddr::from(([255, 255, 255, 255], 9));

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

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::InvalidArgument(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::FailedPrecondition(msg) => (StatusCode::PRECONDITION_FAILED, msg),
            AppError::Internal(msg) => {
                tracing::error!("Internal server error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "An internal server error occurred".to_string(),
                )
            }
            AppError::AlreadyExists(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Database(msg) => {
                tracing::error!("Database error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "An internal server error occurred".to_string(),
                )
            }
        };

        let body = Json(json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}
