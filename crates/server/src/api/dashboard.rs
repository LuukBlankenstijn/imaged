use std::ops::Deref;
use std::sync::Arc;

use derive_more::Constructor;
use imaged_rpc::dashboard::v1 as pb;
use imaged_rpc::dashboard::v1::dashboard_service_server::DashboardService;
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
    async fn get_all_hosts(
        &self,
        req: Request<pb::GetAllHostsRequest>,
    ) -> TonicResult<pb::GetHostsResponse> {
        let request = req.into_inner();
        let hosts = self
            .host_repo
            .get_all(request.group_id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(Response::new(pb::GetHostsResponse { hosts }))
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
            .create(TaskType::Deploy, vec![request.id], Some(request.image_id))
            .await?;
        self.host_registry.send_task(request.id, &task);
        Ok(Response::new(task.into()))
    }

    async fn multicast(&self, req: Request<pb::MulticastHostsRequest>) -> TonicResult {
        let request = req.into_inner();
        let task = self
            .task_repo
            .create(
                TaskType::Multicast,
                request.host_ids.clone(),
                Some(request.image_id),
            )
            .await?;
        self.multicast_manager.notify_new(task.id)?;
        for id in request.host_ids.into_iter() {
            self.host_registry.send_task(id, &task);
        }
        Ok(().into())
    }

    async fn reboot(&self, req: Request<pb::RebootHostsRequest>) -> TonicResult {
        let request = req.into_inner();
        let task = self
            .task_repo
            .create(TaskType::Reboot, request.host_ids.clone(), None)
            .await?;
        for id in request.host_ids.into_iter() {
            self.host_registry.send_task(id, &task);
        }

        Ok(().into())
    }

    async fn get_all_images(&self, _: Request<()>) -> TonicResult<pb::GetImagesResponse> {
        let images = self.image_repo.get_all().await?;

        let resp = pb::GetImagesResponse {
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

        let task = self
            .task_repo
            .create(TaskType::Capture, vec![request.host_id], Some(image.id))
            .await?;
        self.host_registry.send_task(request.host_id, &task);

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
        self.image_service.clear_image_data(request.id).await?;
        Ok(Response::new(()))
    }

    async fn get_all_tasks(&self, _: Request<()>) -> TonicResult<pb::GetTasksResponse> {
        Ok(Response::new(pb::GetTasksResponse {
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
        if let Some(image_id) = task.image_id
            && task.task_type == TaskType::Capture
        {
            self.image_repo
                .mark_faulted(image_id, "Capture task was cancelled by user")
                .await?;
        }
        if let Some(&host_id) = task.hosts.first() {
            self.host_registry.cancel_task(host_id, task.id);
        }
        Ok(Response::new(()))
    }

    async fn retry_task(&self, req: Request<pb::Id>) -> TonicResult {
        let request = req.into_inner();
        let task = self.task_repo.get(request.id).await?;
        if !(task.state.is_cancelled() || task.state.is_failed()) {
            return Err(AppError::InvalidArgument(format!(
                "cannot retry task {}, task is not failed or cancelled",
                request.id
            ))
            .into());
        }
        if task.hosts.len() == 0 {
            return Err(AppError::InvalidArgument(format!(
                "cannot retry task {}, hosts are deleted",
                request.id
            ))
            .into());
        };
        if task.image_id.is_none() {
            return Err(AppError::InvalidArgument(format!(
                "cannot retry task {}, image is deleted",
                request.id
            ))
            .into());
        };
        self.task_repo.retry(task.id).await?;
        if task.task_type == TaskType::Multicast {
            self.multicast_manager.notify_new(task.id)?;
        }
        for host_id in &task.hosts {
            if let Some(next_task) = self.task_repo.get_next(*host_id).await?
                && next_task.id == task.id
            {
                self.host_registry.send_task(*host_id, &task);
            }
        }
        Ok(Response::new(()))
    }

    async fn create_group(&self, req: Request<pb::CreateGroupRequest>) -> TonicResult<pb::Group> {
        let request = req.into_inner();
        Ok(Response::new(
            self.group_repo
                .create_group(&request.name, &request.host_ids)
                .await?
                .into(),
        ))
    }

    async fn update_group_name(
        &self,
        req: Request<pb::UpdateNameRequest>,
    ) -> TonicResult<pb::Group> {
        let request = req.into_inner();
        Ok(Response::new(
            self.group_repo
                .update_name(request.id, &request.new_name)
                .await?
                .into(),
        ))
    }

    async fn get_all_groups(&self, _: Request<()>) -> TonicResult<pb::GetGroupsResponse> {
        Ok(Response::new(pb::GetGroupsResponse {
            groups: self
                .group_repo
                .get_all()
                .await?
                .into_iter()
                .map(Into::into)
                .collect(),
        }))
    }

    async fn update_group_memberships(
        &self,
        req: Request<pb::UpdateGroupRequest>,
    ) -> TonicResult<pb::Group> {
        let request = req.into_inner();
        Ok(Response::new(
            self.group_repo
                .update_group_members(request.id, &request.host_ids)
                .await?
                .into(),
        ))
    }

    async fn delete_group(&self, req: Request<pb::Id>) -> TonicResult {
        let request = req.into_inner();
        self.group_repo.delete(request.id).await?;
        Ok(().into())
    }
}
