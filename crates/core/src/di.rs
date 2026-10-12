use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;

use derive_more::Constructor;
use injectable::{install_injectable_container, register_injectable};
use parking_lot::RwLock;

use crate::AgentLiveness;
use crate::domain::group::GroupRepository;
use crate::domain::host::HostRepository;
use crate::domain::image::ImageRepository;
use crate::domain::script::ScriptRepository;
use crate::domain::task::TaskRepository;
use crate::multicast::MulticastManager;
use crate::registry::HostRegistry;
use crate::service::archive::ArchiveService;
use crate::service::image::ImageService;

#[allow(clippy::too_many_arguments)]
#[derive(Clone, Constructor)]
pub struct DIContainer {
    pub host_repo: Arc<dyn HostRepository>,
    pub image_repo: Arc<dyn ImageRepository>,
    pub task_repo: Arc<dyn TaskRepository>,
    pub group_repo: Arc<dyn GroupRepository>,
    pub script_repo: Arc<dyn ScriptRepository>,
    pub host_registry: Arc<HostRegistry>,
    pub image_service: Arc<ImageService>,
    pub archive_service: Arc<ArchiveService>,
    pub multicast_manager: Arc<MulticastManager>,
    pub machine_ip: Arc<RwLock<Option<IpAddr>>>,
    pub agent_liveness: AgentLiveness,
    pub script_proxy: Option<Arc<str>>,
    pub script_variables: Arc<BTreeMap<String, String>>,
}

install_injectable_container!(DIContainer);

register_injectable!(HostRepo(Arc<dyn HostRepository>), host_repo);
register_injectable!(ImageRepo(Arc<dyn ImageRepository>), image_repo);
register_injectable!(TaskRepo(Arc<dyn TaskRepository>), task_repo);
register_injectable!(GroupRepo(Arc<dyn GroupRepository>), group_repo);
register_injectable!(ScriptRepo(Arc<dyn ScriptRepository>), script_repo);
register_injectable!(Registry(Arc<HostRegistry>), host_registry);
register_injectable!(ImageSvc(Arc<ImageService>), image_service);
register_injectable!(ArchiveSvc(Arc<ArchiveService>), archive_service);
register_injectable!(MulticastMgr(Arc<MulticastManager>), multicast_manager);
register_injectable!(MachineIp(Arc<RwLock<Option<IpAddr>>>), machine_ip);
register_injectable!(Liveness(AgentLiveness), agent_liveness);
register_injectable!(ScriptProxy(Option<Arc<str>>), script_proxy);
register_injectable!(
    ScriptVariables(Arc<BTreeMap<String, String>>),
    script_variables
);
