use std::net::SocketAddr;
use std::sync::Arc;

use derive_more::Constructor;
use inject::{install_injectable_container, register_injectable};

use crate::domain::group::GroupRepository;
use crate::domain::host::HostRepository;
use crate::domain::image::ImageRepository;
use crate::domain::task::TaskRepository;
use crate::multicast::MulticastManager;
use crate::registry::HostRegistry;
use crate::service::image::ImageService;

#[derive(Clone, Constructor)]
pub struct DIContainer {
    pub host_repo: Arc<dyn HostRepository>,
    pub image_repo: Arc<dyn ImageRepository>,
    pub task_repo: Arc<dyn TaskRepository>,
    pub group_repo: Arc<dyn GroupRepository>,
    pub host_registry: Arc<HostRegistry>,
    pub image_service: Arc<ImageService>,
    pub multicast_manager: Arc<MulticastManager>,
    pub bind_address: SocketAddr,
}

install_injectable_container!(DIContainer);

register_injectable!(HostRepo(Arc<dyn HostRepository>), host_repo);
register_injectable!(ImageRepo(Arc<dyn ImageRepository>), image_repo);
register_injectable!(TaskRepo(Arc<dyn TaskRepository>), task_repo);
register_injectable!(GroupRepo(Arc<dyn GroupRepository>), group_repo);
register_injectable!(Registry(Arc<HostRegistry>), host_registry);
register_injectable!(ImageSvc(Arc<ImageService>), image_service);
register_injectable!(MulticastMgr(Arc<MulticastManager>), multicast_manager);
register_injectable!(BindAddress(SocketAddr), bind_address);
