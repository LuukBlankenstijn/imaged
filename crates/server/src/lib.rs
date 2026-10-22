#![cfg(feature = "server")]

use std::{
    collections::BTreeMap,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{Router, http::Uri, routing::get};
use dioxus::server::{DioxusRouterExt, FullstackState, ServeConfig};
pub use imaged_api_ui::model::{Image, TaskProgress};
use imaged_core::{
    self as core,
    di::{DIContainer, MachineIp},
    progress::Snapshot,
    registry::HostRegistry,
};
use imaged_model::error::AppError;
pub use imaged_model::script::ScriptInput;
use imaged_web::App;
use tokio::io::AsyncRead;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

const STOP_GRACE_PERIOD: Duration = Duration::from_secs(3);

static OPENED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Config {
    pub data_dir: PathBuf,
    pub multicast_interface: String,
    pub script_proxy: Option<String>,
    pub script_variables: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Listeners {
    pub machine: SocketAddr,
    pub public_url: String,
    pub tftp: Option<SocketAddr>,
    pub proxy: Option<SocketAddr>,
    pub dashboard: Dashboard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Dashboard {
    Off,
    OnMachineListener,
    Separate(SocketAddr),
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>, multicast_interface: impl Into<String>) -> Self {
        Self {
            data_dir: data_dir.into(),
            multicast_interface: multicast_interface.into(),
            script_proxy: None,
            script_variables: BTreeMap::new(),
        }
    }
}

impl Listeners {
    pub fn new(machine: SocketAddr, public_url: impl Into<String>) -> Self {
        Self {
            machine,
            public_url: public_url.into(),
            tftp: None,
            proxy: None,
            dashboard: Dashboard::Off,
        }
    }
}

pub struct Imaged {
    container: DIContainer,
}

pub struct Server {
    shutdown: CancellationToken,
    abort: CancellationToken,
    listeners: JoinSet<io::Result<()>>,
    registry: Arc<HostRegistry>,
    _serving: ServingClaim,
}

struct ServingClaim(MachineIp);

pub struct TaskProgressFeed(watch::Receiver<Snapshot>);

impl Imaged {
    pub async fn open(config: Config) -> io::Result<Self> {
        if OPENED.swap(true, Ordering::AcqRel) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "imaged is already open in this process",
            ));
        }
        let opened = Self::load(config).await;
        if opened.is_err() {
            OPENED.store(false, Ordering::Release);
        }
        opened
    }

    async fn load(config: Config) -> io::Result<Self> {
        let images_dir = config.data_dir.join("images");
        tokio::fs::create_dir_all(&images_dir).await?;
        let pool = core::setup_database(&config.data_dir.join("imaged.db"))
            .await
            .map_err(io::Error::other)?;
        let container = core::build_di_container(
            pool,
            images_dir.to_string_lossy().into_owned(),
            config.multicast_interface,
            config.script_proxy,
            config.script_variables,
        )
        .await
        .map_err(io::Error::other)?;
        core::di::init_container(container.clone());
        Ok(Self { container })
    }

    pub async fn serve(&self, listeners: Listeners) -> io::Result<Server> {
        validate_public_url(&listeners.public_url)?;
        let serving = ServingClaim::take(&self.container, listeners.machine.ip())?;

        let machine_router = machine_router(&listeners.public_url);
        let (machine_router, separate_dashboard) = match listeners.dashboard {
            Dashboard::Off => (machine_router, None),
            Dashboard::OnMachineListener => (machine_router.merge(dashboard_router()?), None),
            Dashboard::Separate(address) => (machine_router, Some((address, dashboard_router()?))),
        };

        let abort = CancellationToken::new();
        let machine = core::listener::bind(listeners.machine, abort.clone()).await?;
        let dashboard = match separate_dashboard {
            Some((address, router)) => {
                Some((core::listener::bind(address, abort.clone()).await?, router))
            }
            None => None,
        };
        let tftp = match listeners.tftp {
            Some(address) => Some(imaged_boot::tftp::bind(address).await?),
            None => None,
        };
        let proxy = match listeners.proxy {
            Some(address) => {
                Some(imaged_proxy::Proxy::bind(imaged_proxy::Config::new(address)).await?)
            }
            None => None,
        };

        let mut server = Server {
            shutdown: CancellationToken::new(),
            abort,
            listeners: JoinSet::new(),
            registry: self.container.host_registry.clone(),
            _serving: serving,
        };
        server.spawn(|shutdown| core::serve(machine, machine_router, shutdown));
        if let Some((listener, router)) = dashboard {
            server.spawn(|shutdown| core::serve(listener, router, shutdown));
        }
        if let Some(tftp) = tftp {
            server.spawn(|shutdown| tftp.serve(shutdown));
        }
        if let Some(proxy) = proxy {
            server.spawn(|shutdown| async move {
                tokio::select! {
                    () = proxy.serve() => Err(io::Error::other("the proxy stopped accepting connections")),
                    () = shutdown => Ok(()),
                }
            });
        }

        tracing::info!(
            machine = %listeners.machine,
            public_url = %listeners.public_url,
            dashboard = ?listeners.dashboard,
            tftp = ?listeners.tftp,
            proxy = ?listeners.proxy,
            "imaged serving"
        );
        Ok(server)
    }

    pub async fn import_archive(
        &self,
        archive: impl AsyncRead + Unpin + Send,
    ) -> io::Result<Image> {
        let image = self
            .container
            .archive_service
            .import(archive)
            .await
            .map_err(into_io_error)?;
        Ok(image.into())
    }

    pub async fn script_inputs(&self, image_id: i64) -> io::Result<Vec<ScriptInput>> {
        let scripts = self
            .container
            .script_repo
            .get_for_image(image_id)
            .await
            .map_err(into_io_error)?;
        Ok(scripts
            .into_iter()
            .flat_map(|script| script.inputs)
            .collect())
    }

    pub fn connected_hosts(&self) -> Vec<i64> {
        self.container.host_registry.connected_hosts()
    }

    pub fn task_progress(&self) -> Vec<TaskProgress> {
        let tasks = self.container.progress_board.tasks();
        tasks.into_iter().map(Into::into).collect()
    }

    pub fn watch_task_progress(&self) -> TaskProgressFeed {
        let mut snapshots = self.container.progress_board.watch();
        snapshots.mark_changed();
        TaskProgressFeed(snapshots)
    }
}

impl TaskProgressFeed {
    pub async fn next(&mut self) -> Option<Vec<TaskProgress>> {
        self.0.changed().await.ok()?;
        let tasks = self.0.borrow_and_update().tasks();
        Some(tasks.into_iter().map(Into::into).collect())
    }
}

impl Server {
    pub async fn failed(&self) {
        self.shutdown.cancelled().await
    }

    pub async fn stop(mut self) -> io::Result<()> {
        self.shutdown.cancel();
        self.registry.disconnect_all();
        let mut outcome = Ok(());
        if tokio::time::timeout(STOP_GRACE_PERIOD, self.drain(&mut outcome))
            .await
            .is_err()
        {
            tracing::warn!(
                grace_period = ?STOP_GRACE_PERIOD,
                "open connections outlived the grace period, cutting them off"
            );
            self.abort.cancel();
            let _ = tokio::time::timeout(STOP_GRACE_PERIOD, self.drain(&mut outcome)).await;
        }
        outcome
    }

    async fn drain(&mut self, outcome: &mut io::Result<()>) {
        while let Some(joined) = self.listeners.join_next().await {
            let result = joined.map_err(io::Error::other).and_then(|served| served);
            if outcome.is_ok() {
                *outcome = result;
            }
        }
    }

    fn spawn<F>(&mut self, serve: impl FnOnce(WaitForCancellationFutureOwned) -> F)
    where
        F: Future<Output = io::Result<()>> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        let served = serve(shutdown.clone().cancelled_owned());
        self.listeners.spawn(async move {
            let result = served.await;
            if let Err(error) = &result {
                tracing::error!(%error, "listener failed, stopping imaged");
            }
            shutdown.cancel();
            result
        });
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.abort.cancel();
        self.registry.disconnect_all();
    }
}

impl ServingClaim {
    fn take(container: &DIContainer, ip: IpAddr) -> io::Result<Self> {
        let mut serving = container.machine_ip.write();
        if serving.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "imaged is already serving",
            ));
        }
        *serving = Some(ip);
        Ok(Self(MachineIp(container.machine_ip.clone())))
    }
}

impl Drop for ServingClaim {
    fn drop(&mut self) {
        *self.0.write() = None;
    }
}

#[doc(hidden)]
pub fn machine_router(public_url: &str) -> Router {
    imaged_boot::pxe::router(public_url).merge(imaged_api_agent::router())
}

fn validate_public_url(public_url: &str) -> io::Result<()> {
    let valid = public_url.parse::<Uri>().is_ok_and(|uri| {
        matches!(uri.scheme_str(), Some("http" | "https"))
            && uri.host().is_some_and(|host| !host.is_empty())
    });
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("public_url {public_url:?} is not an http:// or https:// URL with a host"),
        ))
    }
}

fn into_io_error(error: AppError) -> io::Error {
    let kind = match &error {
        AppError::NotFound(_) => io::ErrorKind::NotFound,
        AppError::InvalidArgument(_) => io::ErrorKind::InvalidInput,
        AppError::AlreadyExists(_) => io::ErrorKind::AlreadyExists,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}

fn dashboard_router() -> io::Result<Router> {
    let assets = match std::env::var_os("DIOXUS_PUBLIC_PATH") {
        Some(path) => PathBuf::from(path),
        None => std::env::current_exe()?.with_file_name("public"),
    };
    std::fs::read_dir(&assets).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("dashboard assets in {}: {e}", assets.display()),
        )
    })?;
    Ok(imaged_api_ui::router()
        .serve_static_assets()
        .fallback(get(FullstackState::render_handler))
        .with_state(FullstackState::new(ServeConfig::new(), App)))
}
