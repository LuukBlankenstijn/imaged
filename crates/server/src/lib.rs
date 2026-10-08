#![cfg(feature = "server")]

use std::{
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

use axum::{Router, routing::get};
use dioxus::server::{DioxusRouterExt, FullstackState, ServeConfig, ServerFunction};
use imaged_core::{
    self as core,
    di::{DIContainer, MachineIp},
    registry::HostRegistry,
};
use imaged_web::App;
use tokio::task::JoinSet;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

use imaged_api_client as _;
use imaged_api_ui as _;

const CLIENT_API_PREFIX: &str = "/api/client";
const STOP_GRACE_PERIOD: Duration = Duration::from_secs(3);

static OPENED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub multicast_interface: String,
}

#[derive(Clone, Debug)]
pub struct Listeners {
    pub machine: SocketAddr,
    pub public_url: String,
    pub tftp: Option<SocketAddr>,
    pub dashboard: Dashboard,
}

#[derive(Clone, Copy, Debug)]
pub enum Dashboard {
    Off,
    OnMachineListener,
    Separate(SocketAddr),
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
        )
        .await
        .map_err(io::Error::other)?;
        core::di::init_container(container.clone());
        Ok(Self { container })
    }

    pub async fn serve(&self, listeners: Listeners) -> io::Result<Server> {
        let serving = ServingClaim::take(&self.container, listeners.machine.ip())?;

        let machine_router = core::pxe::router(&listeners.public_url)
            .merge(server_fns(is_client_api).with_state(FullstackState::headless()));
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
            Some(address) => Some(core::tftp::bind(address).await?),
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

        tracing::info!(
            machine = %listeners.machine,
            public_url = %listeners.public_url,
            dashboard = ?listeners.dashboard,
            tftp = ?listeners.tftp,
            "imaged serving"
        );
        Ok(server)
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

fn is_client_api(path: &str) -> bool {
    path.starts_with(CLIENT_API_PREFIX)
}

fn server_fns(keep: impl Fn(&str) -> bool) -> Router<FullstackState> {
    ServerFunction::collect()
        .into_iter()
        .filter(|f| keep(f.path()))
        .fold(Router::new(), |router, f| {
            router.route(f.path(), f.method_router())
        })
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
    Ok(server_fns(|path| !is_client_api(path))
        .serve_static_assets()
        .fallback(get(FullstackState::render_handler))
        .with_state(FullstackState::new(ServeConfig::new(), App)))
}
