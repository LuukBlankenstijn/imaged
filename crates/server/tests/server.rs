#![cfg(feature = "server")]

use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use imaged_server::{Config, Dashboard, Imaged, Listeners};
use reqwest::StatusCode;

const PUBLIC_URL: &str = "http://imaged.test:8080";

struct DataDir(PathBuf);

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

fn listeners(machine: SocketAddr, tftp: SocketAddr, dashboard: Dashboard) -> Listeners {
    Listeners {
        machine,
        public_url: PUBLIC_URL.to_string(),
        tftp: Some(tftp),
        dashboard,
    }
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
            .post(format!("http://{address}/api/client/stream/disconnect"))
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
    let public_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("public");
    std::fs::create_dir_all(public_dir).unwrap();
    let probe = Probe::new();

    let imaged = Imaged::open(Config {
        data_dir: data_dir.0.join("nested"),
        multicast_interface: "lo".to_string(),
    })
    .await
    .unwrap();
    assert!(data_dir.0.join("nested/imaged.db").is_file());
    assert!(data_dir.0.join("nested/images").is_dir());

    let tftp = free_udp_address();
    let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let unstarted = free_address();
    let refused = imaged
        .serve(listeners(
            unstarted,
            tftp,
            Dashboard::Separate(occupied.local_addr().unwrap()),
        ))
        .await;
    assert_eq!(
        refused.err().map(|error| error.kind()),
        Some(std::io::ErrorKind::AddrInUse)
    );
    assert!(probe.refuses_connections(unstarted).await);

    let headless = free_address();
    let server = imaged
        .serve(listeners(headless, tftp, Dashboard::Off))
        .await
        .unwrap();
    let script = probe.boot_script(headless).await.unwrap();
    assert!(
        script.contains(&format!("chain {PUBLIC_URL}/boot/manifest.ipxe?")),
        "{script}"
    );
    assert!(probe.serves_agent_api(headless).await);
    assert!(!probe.serves_dashboard_api(headless).await);
    imaged_api_ui::hosts::wake_on_lan(Vec::new()).await.unwrap();

    let stopping = Instant::now();
    server.stop().await.unwrap();
    assert!(stopping.elapsed() < Duration::from_secs(1));
    assert!(probe.refuses_connections(headless).await);
    assert!(imaged_api_ui::hosts::wake_on_lan(Vec::new()).await.is_err());

    let machine = free_address();
    let dashboard = free_address();
    let server = imaged
        .serve(listeners(machine, tftp, Dashboard::Separate(dashboard)))
        .await
        .unwrap();
    assert!(probe.boot_script(machine).await.is_some());
    assert!(probe.serves_agent_api(machine).await);
    assert!(!probe.serves_dashboard_api(machine).await);
    assert!(probe.boot_script(dashboard).await.is_none());
    assert!(!probe.serves_agent_api(dashboard).await);
    assert!(probe.serves_dashboard_api(dashboard).await);

    let endless_stream = probe
        .0
        .get(format!("http://{dashboard}/api/ui/multicast-progress"))
        .send()
        .await
        .unwrap();
    assert_eq!(endless_stream.status(), StatusCode::OK);
    let stopping = Instant::now();
    server.stop().await.unwrap();
    assert!(stopping.elapsed() < Duration::from_secs(5));
    assert!(probe.refuses_connections(machine).await);
    assert!(probe.refuses_connections(dashboard).await);
}
