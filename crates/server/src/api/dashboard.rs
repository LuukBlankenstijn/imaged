use std::sync::Arc;

use derive_more::Constructor;
use imaged_rpc::dashboard::v1::dashboard_service_server::DashboardService;
use imaged_rpc::dashboard::v1::{self as pb, GetAllHostsResponse};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response};

use crate::domain::host::{Host, HostRepository};
use crate::registry::{HostConnectionEvent, HostRegistry};

#[derive(Constructor)]
pub struct DashboardHandler {
    host_repo: Arc<dyn HostRepository>,
    host_registry: Arc<HostRegistry>,
}

impl From<pb::Host> for Host {
    fn from(value: pb::Host) -> Self {
        Self::new(
            value.id,
            value.name,
            value.mac_address,
            value.disk_size_bytes,
        )
    }
}

impl From<Host> for pb::Host {
    fn from(value: Host) -> Self {
        Self {
            id: value.id,
            mac_address: value.mac_address,
            name: value.name,
            disk_size_bytes: value.disk_size,
        }
    }
}

impl From<HostConnectionEvent> for pb::HostConnectionEvent {
    fn from(value: HostConnectionEvent) -> Self {
        Self {
            id: value.id,
            connected: value.connected,
        }
    }
}

#[tonic::async_trait]
impl DashboardService for DashboardHandler {
    async fn get_all_hosts(
        &self,
        _: Request<()>,
    ) -> Result<Response<pb::GetAllHostsResponse>, tonic::Status> {
        let hosts = self
            .host_repo
            .get_all()
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        let response = GetAllHostsResponse { hosts };
        Ok(response.into())
    }

    async fn update_host_name(
        &self,
        req: Request<pb::UpdateHostNameRequest>,
    ) -> Result<Response<pb::Host>, tonic::Status> {
        let request = req.into_inner();
        let host = self
            .host_repo
            .update_name(request.id, request.new_name)
            .await?;
        Ok(Response::new(host.into()))
    }

    type ConnectionStateStream = ReceiverStream<Result<pb::HostConnectionEvent, tonic::Status>>;

    async fn connection_state(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::ConnectionStateStream>, tonic::Status> {
        let mut update_stream = self.host_registry.subscribe_state();

        let initial_state = self.host_registry.get_current_state();

        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            // Send all initial state entries
            for entry in initial_state {
                let message = pb::HostConnectionEvent {
                    id: entry.id,
                    connected: entry.connected,
                };

                if tx.send(Ok(message)).await.is_err() {
                    return;
                }
            }

            while let Ok(update) = update_stream.recv().await {
                if tx.send(Ok(update.into())).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
