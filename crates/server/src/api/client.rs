use std::sync::Arc;

use derive_more::Constructor;
use imaged_rpc::client::v1 as pb;
use imaged_rpc::client::v1::client_service_server::ClientService;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response};

use crate::{domain::host::HostRepository, registry::HostRegistry};

#[derive(Clone, Constructor)]
pub struct ClientHandler {
    host_repo: Arc<dyn HostRepository>,
    host_registry: Arc<HostRegistry>,
}

#[tonic::async_trait]
impl ClientService for ClientHandler {
    type StartStreamStream = ReceiverStream<Result<pb::Task, tonic::Status>>;

    async fn start_stream(
        &self,
        req: Request<pb::StartStreamRequest>,
    ) -> Result<Response<Self::StartStreamStream>, tonic::Status> {
        let request = req.into_inner();
        let host = self
            .host_repo
            .upsert_host(request.mac.clone(), request.disk_size_bytes)
            .await?;

        let (tx, rx) = mpsc::channel(8);

        let mut registration = self.host_registry.register(host.id)?;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_event = registration.receiver.recv() => {
                        match maybe_event {
                            Some(_event) => {
                                let task = pb::Task {};
                                if tx.send(Ok(task)).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }

                    // grpc channel closed
                    _ = tx.closed() => {
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
