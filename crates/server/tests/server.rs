#![cfg(feature = "server")]

use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use imaged_api_ui::model::TaskProgress;
use imaged_model::AgentEvent;
use imaged_model::agent::{MAC_HEADER, route};
use imaged_model::progress::{HostProgress, Stage};
use imaged_server::{Config, Dashboard, Imaged, Listeners, TaskProgressFeed};
use reqwest::StatusCode;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

const PUBLIC_URL: &str = "http://imaged.test:8080";
const PROMPTLY: Duration = Duration::from_secs(10);

type AgentStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct DataDir(PathBuf);

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(data_dir: &Path) -> Config {
    Config::new(data_dir, "lo")
}

fn free_address() -> SocketAddr {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
}

fn free_udp_address() -> SocketAddr {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
}

fn listeners(machine: SocketAddr, tftp: Option<SocketAddr>, dashboard: Dashboard) -> Listeners {
    let mut listeners = Listeners::new(machine, PUBLIC_URL);
    listeners.tftp = tftp;
    listeners.dashboard = dashboard;
    listeners
}

async fn connect_agent(address: SocketAddr) -> AgentStream {
    let mut request = format!("ws://{address}{}?disk_size_bytes=1", route::STREAM)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(MAC_HEADER, "02:00:00:00:00:01".parse().unwrap());
    connect_async(request).await.unwrap().0
}

async fn closes_promptly(agent: &mut AgentStream) -> bool {
    tokio::time::timeout(PROMPTLY, async {
        while let Some(Ok(message)) = agent.next().await {
            if message.is_close() {
                break;
            }
        }
    })
    .await
    .is_ok()
}

async fn next_promptly(progress: &mut TaskProgressFeed) -> Vec<TaskProgress> {
    tokio::time::timeout(PROMPTLY, progress.next())
        .await
        .unwrap()
        .unwrap()
}

async fn ends_promptly(mut response: reqwest::Response) -> bool {
    tokio::time::timeout(PROMPTLY, async {
        while let Ok(Some(_)) = response.chunk().await {}
    })
    .await
    .is_ok()
}

fn error_kind<T>(result: std::io::Result<T>) -> Option<ErrorKind> {
    result.err().map(|error| error.kind())
}

struct Probe(reqwest::Client);

impl Probe {
    fn new() -> Self {
        Self(
            reqwest::Client::builder()
                .pool_max_idle_per_host(0)
                .build()
                .unwrap(),
        )
    }

    async fn boot_script(&self, address: SocketAddr) -> Option<String> {
        let response = self
            .0
            .get(format!("http://{address}/boot/boot.ipxe"))
            .send()
            .await
            .unwrap();
        let body = response.text().await.unwrap();
        body.starts_with("#!ipxe").then_some(body)
    }

    async fn serves_agent_api(&self, address: SocketAddr) -> bool {
        let response = self
            .0
            .post(format!("http://{address}{}", route::DISCONNECT))
            .send()
            .await
            .unwrap();
        response.status() == StatusCode::BAD_REQUEST
            && response.text().await.unwrap().contains("missing mac")
    }

    async fn serves_dashboard_api(&self, address: SocketAddr) -> bool {
        let response = self
            .0
            .get(format!("http://{address}/api/ui/images"))
            .send()
            .await
            .unwrap();
        response.status() == StatusCode::OK
    }

    async fn dashboard_progress(&self, address: SocketAddr) -> reqwest::Response {
        self.0
            .get(format!("http://{address}/api/ui/task-progress"))
            .send()
            .await
            .unwrap()
    }

    async fn wake_nobody(&self, address: SocketAddr, headers: &[(&str, &str)]) -> StatusCode {
        let mut request = self
            .0
            .post(format!("http://{address}/api/ui/hosts/wake"))
            .body(r#"{"host_ids":[]}"#);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send().await.unwrap().status()
    }

    async fn refuses_connections(&self, address: SocketAddr) -> bool {
        self.0
            .get(format!("http://{address}/boot/boot.ipxe"))
            .send()
            .await
            .is_err_and(|error| error.is_connect())
    }
}

#[tokio::test]
async fn imaged_serves_machines_on_the_requested_listeners_and_restarts_on_new_addresses() {
    let data_dir =
        DataDir(std::env::temp_dir().join(format!("imaged-server-handle-{}", std::process::id())));
    let dashboard_assets = data_dir.0.join("public");
    unsafe { std::env::set_var("DIOXUS_PUBLIC_PATH", &dashboard_assets) };
    let probe = Probe::new();

    let blocked = data_dir.0.join("blocked");
    std::fs::create_dir_all(&data_dir.0).unwrap();
    std::fs::write(&blocked, b"").unwrap();
    assert!(Imaged::open(config(&blocked)).await.is_err());
    let imaged = Imaged::open(config(&data_dir.0.join("nested")))
        .await
        .unwrap();
    assert!(data_dir.0.join("nested/imaged.db").is_file());
    assert!(data_dir.0.join("nested/images").is_dir());
    let second = data_dir.0.join("second");
    assert_eq!(
        error_kind(Imaged::open(config(&second)).await),
        Some(ErrorKind::AlreadyExists)
    );
    assert!(!second.exists());

    let unstarted = free_address();
    assert_eq!(
        error_kind(
            imaged
                .serve(listeners(unstarted, None, Dashboard::OnMachineListener))
                .await
        ),
        Some(ErrorKind::NotFound)
    );
    assert!(probe.refuses_connections(unstarted).await);
    let mut schemeless = listeners(unstarted, None, Dashboard::Off);
    schemeless.public_url = "imaged.test:8080".to_string();
    assert_eq!(
        error_kind(imaged.serve(schemeless).await),
        Some(ErrorKind::InvalidInput)
    );
    assert!(probe.refuses_connections(unstarted).await);
    std::fs::create_dir_all(&dashboard_assets).unwrap();

    let tftp = free_udp_address();
    let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    assert_eq!(
        error_kind(
            imaged
                .serve(listeners(
                    unstarted,
                    Some(tftp),
                    Dashboard::Separate(occupied.local_addr().unwrap()),
                ))
                .await
        ),
        Some(ErrorKind::AddrInUse)
    );
    assert!(probe.refuses_connections(unstarted).await);

    let headless = free_address();
    let server = imaged
        .serve(listeners(headless, Some(tftp), Dashboard::Off))
        .await
        .unwrap();
    let script = probe.boot_script(headless).await.unwrap();
    assert!(
        script.contains(&format!("chain {PUBLIC_URL}/boot/manifest.ipxe?")),
        "{script}"
    );
    assert!(probe.serves_agent_api(headless).await);
    assert!(!probe.serves_dashboard_api(headless).await);
    assert_eq!(
        error_kind(
            imaged
                .serve(listeners(free_address(), None, Dashboard::Off))
                .await
        ),
        Some(ErrorKind::AlreadyExists)
    );
    imaged_api_ui::hosts::wake_on_lan(Vec::new()).await.unwrap();

    let mut agent = connect_agent(headless).await;
    assert_eq!(imaged.connected_hosts().len(), 1);
    let mut progress = imaged.watch_task_progress();
    assert_eq!(progress.next().await, Some(Vec::new()));
    let reported = HostProgress {
        task_id: 7,
        fraction: 0.5,
        stage: Stage::Finishing,
    };
    let event = serde_json::to_string(&AgentEvent::from(reported.clone())).unwrap();
    agent.send(Message::text(event)).await.unwrap();
    let tasks = next_promptly(&mut progress).await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, 7);
    assert_eq!(tasks[0].hosts.values().collect::<Vec<_>>(), [&reported]);
    assert_eq!(imaged.task_progress(), tasks);
    let stopping = Instant::now();
    server.stop().await.unwrap();
    assert!(stopping.elapsed() < Duration::from_secs(3));
    assert!(closes_promptly(&mut agent).await);
    assert!(imaged.connected_hosts().is_empty());
    assert!(next_promptly(&mut progress).await.is_empty());
    assert!(probe.refuses_connections(headless).await);
    assert!(imaged_api_ui::hosts::wake_on_lan(Vec::new()).await.is_err());

    let machine = free_address();
    let dashboard = free_address();
    let server = imaged
        .serve(listeners(
            machine,
            Some(tftp),
            Dashboard::Separate(dashboard),
        ))
        .await
        .unwrap();
    assert!(probe.boot_script(machine).await.is_some());
    assert!(probe.serves_agent_api(machine).await);
    assert!(!probe.serves_dashboard_api(machine).await);
    assert!(probe.boot_script(dashboard).await.is_none());
    assert!(!probe.serves_agent_api(dashboard).await);
    assert!(probe.serves_dashboard_api(dashboard).await);
    let json = ("content-type", "application/json");
    assert_eq!(
        probe
            .wake_nobody(dashboard, &[json, ("sec-fetch-site", "same-origin")])
            .await,
        StatusCode::OK
    );
    assert_eq!(
        probe
            .wake_nobody(dashboard, &[json, ("origin", "http://evil.example")])
            .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        probe
            .wake_nobody(dashboard, &[("content-type", "text/plain")])
            .await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    let endless_stream = probe.dashboard_progress(dashboard).await;
    let stopping = Instant::now();
    server.stop().await.unwrap();
    assert!(stopping.elapsed() < Duration::from_secs(5));
    assert!(ends_promptly(endless_stream).await);
    assert!(probe.refuses_connections(machine).await);
    assert!(probe.refuses_connections(dashboard).await);

    let combined = free_address();
    let server = imaged
        .serve(listeners(combined, None, Dashboard::OnMachineListener))
        .await
        .unwrap();
    assert!(probe.boot_script(combined).await.is_some());
    assert!(probe.serves_agent_api(combined).await);
    assert!(probe.serves_dashboard_api(combined).await);
    server.stop().await.unwrap();
    assert!(probe.refuses_connections(combined).await);
}
