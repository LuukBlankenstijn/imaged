#![cfg(feature = "server")]

use std::{future::Future, io, net::SocketAddr, path::PathBuf, time::Duration};

use axum::{Router, routing::get};
use dioxus::server::{DioxusRouterExt, FullstackState, ServeConfig, ServerFunction};
use imaged_core::{self as core, BoxError, di::MachineIp};
use imaged_web::App;
use tokio::task::JoinSet;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

use imaged_api_client as _;
use imaged_api_ui as _;

const CLIENT_API_PREFIX: &str = "/api/client";
const STOP_GRACE_PERIOD: Duration = Duration::from_secs(3);

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
    machine_ip: MachineIp,
}

pub struct Server {
    shutdown: CancellationToken,
    listeners: JoinSet<io::Result<()>>,
    machine_ip: MachineIp,
}

impl Imaged {
    pub async fn open(config: Config) -> Result<Self, BoxError> {
        let images_dir = config.data_dir.join("images");
        tokio::fs::create_dir_all(&images_dir).await?;
        let pool = core::setup_database(&config.data_dir.join("imaged.db")).await?;
        let container = core::build_di_container(
            pool,
            images_dir.to_string_lossy().into_owned(),
            config.multicast_interface,
        )
        .await?;
        let machine_ip = MachineIp(container.machine_ip.clone());
        core::di::init_container(container);
        Ok(Self { machine_ip })
    }

    pub async fn serve(&self, listeners: Listeners) -> io::Result<Server> {
        let machine = core::bind(listeners.machine).await?;
        let dashboard = match listeners.dashboard {
            Dashboard::Separate(address) => Some(core::bind(address).await?),
            Dashboard::Off | Dashboard::OnMachineListener => None,
        };
        let tftp = match listeners.tftp {
            Some(address) => Some(core::tftp::bind(address).await?),
            None => None,
        };

        let machine_router = core::pxe::router(&listeners.public_url)
            .merge(server_fns(is_client_api).with_state(FullstackState::headless()));
        let machine_router = match listeners.dashboard {
            Dashboard::OnMachineListener => machine_router.merge(dashboard_router()),
            Dashboard::Off | Dashboard::Separate(_) => machine_router,
        };

        let mut server = Server {
            shutdown: CancellationToken::new(),
            listeners: JoinSet::new(),
            machine_ip: self.machine_ip.clone(),
        };
        server.spawn(|shutdown| core::serve(machine, machine_router, shutdown));
        if let Some(dashboard) = dashboard {
            server.spawn(|shutdown| core::serve(dashboard, dashboard_router(), shutdown));
        }
        if let Some(tftp) = tftp {
            server.spawn(|shutdown| tftp.serve(shutdown));
        }
        *server.machine_ip.write() = Some(listeners.machine.ip());

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
        let mut outcome = Ok(());
        let drained = tokio::time::timeout(STOP_GRACE_PERIOD, async {
            while let Some(joined) = self.listeners.join_next().await {
                let result = joined.map_err(io::Error::other).and_then(|served| served);
                if outcome.is_ok() {
                    outcome = result;
                }
            }
        })
        .await;
        if drained.is_err() {
            tracing::warn!(
                grace_period = ?STOP_GRACE_PERIOD,
                "open connections outlived the grace period, aborting listeners"
            );
        }
        outcome
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
        *self.machine_ip.write() = None;
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

fn dashboard_router() -> Router {
    server_fns(|path| !is_client_api(path))
        .serve_static_assets()
        .fallback(get(FullstackState::render_handler))
        .with_state(FullstackState::new(ServeConfig::new(), App))
}
