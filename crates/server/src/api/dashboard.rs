use std::ops::Deref;
use std::sync::Arc;

use derive_more::Constructor;
use imaged_rpc::dashboard::v1::dashboard_service_server::DashboardService;
use imaged_rpc::dashboard::v1::{self as pb, GetAllHostsResponse};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response};

use crate::api::HandlerState;
use crate::domain::task::TaskType;
use crate::error::AppError;

mod convert;

type TonicResult<T = ()> = Result<Response<T>, tonic::Status>;

#[derive(Constructor, Clone)]
pub struct DashboardHandler(Arc<HandlerState>);

impl Deref for DashboardHandler {
    type Target = Arc<HandlerState>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[tonic::async_trait]
impl DashboardService for DashboardHandler {
    async fn get_all_hosts(&self, _: Request<()>) -> TonicResult<pb::GetAllHostsResponse> {
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

    async fn update_host_name(&self, req: Request<pb::UpdateNameRequest>) -> TonicResult<pb::Host> {
        let request = req.into_inner();
        let host = self
            .host_repo
            .update_name(request.id, request.new_name)
            .await?;
        Ok(Response::new(host.into()))
    }

    type ConnectionStateStream = ReceiverStream<Result<pb::HostConnectionEvent, tonic::Status>>;

    async fn connection_state(&self, _: Request<()>) -> TonicResult<Self::ConnectionStateStream> {
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

    async fn delete_host(&self, req: Request<pb::Id>) -> TonicResult {
        let id = req.into_inner().id;
        if self.task_repo.get_next(id).await?.is_some() {
            return Err(AppError::InvalidArgument(format!(
                "Host with id {id} has active tasks, finish or cancel those first"
            ))
            .into());
        }
        let _ = self.host_repo.delete(id).await?;
        Ok(Response::new(()))
    }

    async fn deploy(&self, req: Request<pb::DeployHostRequest>) -> TonicResult<pb::Task> {
        let request = req.into_inner();
        let task = self
            .task_repo
            .create(TaskType::Deploy, request.id, request.image_id)
            .await?;
        // TODO: notify host of task
        Ok(Response::new(task.into()))
    }

    async fn get_all_images(&self, _: Request<()>) -> TonicResult<pb::GetAllImagesResponse> {
        let images = self.image_repo.get_all().await?;

        let resp = pb::GetAllImagesResponse {
            images: images.into_iter().map(Into::into).collect(),
        };

        Ok(Response::new(resp))
    }

    async fn update_image_name(
        &self,
        req: Request<pb::UpdateNameRequest>,
    ) -> TonicResult<pb::Image> {
        let request = req.into_inner();
        let image = self
            .image_repo
            .update_name(request.id, request.new_name)
            .await?;
        Ok(Response::new(image.into()))
    }

    async fn create_image(&self, req: Request<pb::CreateImageRequest>) -> TonicResult<pb::Image> {
        let request = req.into_inner();
        let image = self.image_repo.create_image(request.name).await?;

        self.task_repo
            .create(TaskType::Capture, request.host_id, image.id)
            .await?;
        // TODO: notify host of task

        Ok(Response::new(image.into()))
    }

    async fn delete_image(&self, req: Request<pb::Id>) -> TonicResult {
        let request = req.into_inner();
        if !self
            .task_repo
            .get_active_by_image(request.id)
            .await?
            .is_empty()
        {
            return Err(AppError::InvalidArgument(format!(
                "image with id {} has active tasks, first cancel or complete those",
                request.id
            ))
            .into());
        };

        self.image_repo.delete_image(request.id).await?;
        Ok(Response::new(()))
    }

    async fn get_all_tasks(&self, _: Request<()>) -> TonicResult<pb::GetAllTasksResponse> {
        Ok(Response::new(pb::GetAllTasksResponse {
            tasks: self
                .task_repo
                .get_all()
                .await?
                .into_iter()
                .map(Into::into)
                .collect(),
        }))
    }

    async fn cancel_task(&self, req: Request<pb::Id>) -> TonicResult {
        let request = req.into_inner();
        let task = self.task_repo.get(request.id).await?;
        if !(task.state.is_pending() || task.state.is_running()) {
            return Err(AppError::InvalidArgument(format!(
                "cannot cancel task {}, task is not pending or running",
                request.id
            ))
            .into());
        }
        self.task_repo.cancel(task.id).await?;
        // TODO: notify host of cancelation
        Ok(Response::new(()))
    }
}
