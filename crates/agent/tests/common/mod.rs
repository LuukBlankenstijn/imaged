#![allow(dead_code)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use imaged_agent::agent::Agent;
use imaged_agent::server::Server;
use imaged_agent::status::{Identity, Status};
use imaged_model::agent::{
    ImageLayout, ImagePartition, MAC_HEADER, MacAddress, PostInstall, TaskFailure, route,
};
use imaged_model::{AgentEvent, ServerEvent};
use reqwest::Url;

const CHUNK: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    Parttable,
    Partitions,
    PartitionData,
    Finished,
    Failed,
    Disconnect,
    Stream,
}

/// What the stub does after streaming its configured events on each websocket connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StreamBehavior {
    /// Keep reading the socket (so axum auto-replies to pings) until the peer disconnects,
    /// while honouring live [`StubCommand`]s pushed through the control channel.
    #[default]
    HoldOpen,
    /// Close the socket from the server side once the events are delivered.
    Close,
    /// Hold the socket open but never read it, so incoming pings are never answered.
    Silent,
}

/// A live instruction for an open websocket connection, pushed via [`StubServer::send_event`]
/// or [`StubServer::close_connections`].
#[derive(Debug, Clone)]
pub enum StubCommand {
    Send(ServerEvent),
    Ping,
    Close,
}

#[derive(Default, Clone)]
pub struct StubConfig {
    pub parttable: Vec<u8>,
    pub partitions: Vec<ImagePartition>,
    pub partition_data: HashMap<i64, Vec<u8>>,
    pub events: Vec<ServerEvent>,
    pub force_status: HashMap<Route, u16>,
    pub stream_behavior: StreamBehavior,
    pub post_install: HashMap<i64, PostInstall>,
    pub transient_failures: HashMap<Route, usize>,
}

impl StubConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_partition(mut self, number: i64, compressed: Vec<u8>) -> Self {
        self.partition_data.insert(number, compressed);
        self
    }

    pub fn with_parttable(mut self, bytes: Vec<u8>) -> Self {
        self.parttable = bytes;
        self
    }

    pub fn with_partitions(mut self, partitions: Vec<ImagePartition>) -> Self {
        self.partitions = partitions;
        self
    }

    pub fn with_event(mut self, event: ServerEvent) -> Self {
        self.events.push(event);
        self
    }

    pub fn stream_behavior(mut self, behavior: StreamBehavior) -> Self {
        self.stream_behavior = behavior;
        self
    }

    pub fn with_post_install(mut self, task_id: i64, plan: PostInstall) -> Self {
        self.post_install.insert(task_id, plan);
        self
    }

    pub fn force(mut self, route: Route, status: u16) -> Self {
        self.force_status.insert(route, status);
        self
    }

    /// Answer the first `times` requests to `route` with 503 Service Unavailable.
    pub fn fail_transiently(mut self, route: Route, times: usize) -> Self {
        self.transient_failures.insert(route, times);
        self
    }
}

#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub mac: Option<String>,
}

struct AppState {
    cfg: StubConfig,
    transient_failures: Mutex<HashMap<Route, usize>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    finished: Arc<Mutex<Vec<i64>>>,
    failed: Arc<Mutex<Vec<(i64, String)>>>,
    upgrades: Arc<AtomicUsize>,
    control: broadcast::Sender<StubCommand>,
    pongs: Arc<AtomicUsize>,
    agent_events: Arc<Mutex<Vec<AgentEvent>>>,
}

pub struct StubServer {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
    pub finished: Arc<Mutex<Vec<i64>>>,
    pub failed: Arc<Mutex<Vec<(i64, String)>>>,
    upgrades: Arc<AtomicUsize>,
    control: broadcast::Sender<StubCommand>,
    pongs: Arc<AtomicUsize>,
    agent_events: Arc<Mutex<Vec<AgentEvent>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl StubServer {
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn finished(&self) -> Vec<i64> {
        self.finished.lock().unwrap().clone()
    }

    pub fn failed(&self) -> Vec<(i64, String)> {
        self.failed.lock().unwrap().clone()
    }

    /// Number of websocket upgrades the stub has accepted (incremented once per connection,
    /// after a control subscriber for that connection exists).
    pub fn upgrades(&self) -> usize {
        self.upgrades.load(Ordering::SeqCst)
    }

    /// Push a server event to every currently open websocket connection.
    pub fn send_event(&self, event: ServerEvent) {
        let _ = self.control.send(StubCommand::Send(event));
    }

    /// Ask every currently open websocket connection to close from the server side.
    pub fn close_connections(&self) {
        let _ = self.control.send(StubCommand::Close);
    }

    /// Ping every currently open websocket connection. The peer is expected to answer
    /// with a pong, which [`StubServer::pongs`] counts.
    pub fn ping_connections(&self) {
        let _ = self.control.send(StubCommand::Ping);
    }

    /// Number of pong frames the stub has received across all connections.
    pub fn pongs(&self) -> usize {
        self.pongs.load(Ordering::SeqCst)
    }

    /// Every event the agent sent up its websocket, across all connections.
    pub fn agent_events(&self) -> Vec<AgentEvent> {
        self.agent_events.lock().unwrap().clone()
    }
}

impl Drop for StubServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub async fn stub_server(cfg: StubConfig) -> StubServer {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let finished = Arc::new(Mutex::new(Vec::new()));
    let failed = Arc::new(Mutex::new(Vec::new()));
    let upgrades = Arc::new(AtomicUsize::new(0));
    let (control, _) = broadcast::channel(32);
    let pongs = Arc::new(AtomicUsize::new(0));
    let agent_events = Arc::new(Mutex::new(Vec::new()));

    let state = Arc::new(AppState {
        transient_failures: Mutex::new(cfg.transient_failures.clone()),
        cfg,
        requests: requests.clone(),
        finished: finished.clone(),
        failed: failed.clone(),
        upgrades: upgrades.clone(),
        control: control.clone(),
        pongs: pongs.clone(),
        agent_events: agent_events.clone(),
    });

    let app = Router::new()
        .route(route::PARTITION_TABLE, get(parttable))
        .route(route::PARTITIONS, get(partitions))
        .route(route::PARTITION_DATA, get(partition_data))
        .route(route::SCRIPTS, get(post_install))
        .route(route::FINISHED, post(finished_route))
        .route(route::FAILED, post(failed_route))
        .route(route::DISCONNECT, post(disconnect))
        .route(route::STREAM, get(stream))
        .layer(from_fn_with_state(state.clone(), record_mw))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
    let addr = listener.local_addr().expect("stub addr");
    let base_url = format!("http://{addr}");

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    StubServer {
        base_url,
        requests,
        finished,
        failed,
        upgrades,
        control,
        pongs,
        agent_events,
        handle,
    }
}

async fn record_mw(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let mac = req
        .headers()
        .get(MAC_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    state
        .requests
        .lock()
        .unwrap()
        .push(RecordedRequest { method, path, mac });
    next.run(req).await
}

fn forced(s: &AppState, route: Route) -> Option<Response> {
    let transient = s
        .transient_failures
        .lock()
        .unwrap()
        .get_mut(&route)
        .filter(|left| **left > 0)
        .map(|left| {
            *left -= 1;
            StatusCode::SERVICE_UNAVAILABLE.as_u16()
        });
    transient
        .or_else(|| s.cfg.force_status.get(&route).copied())
        .map(|code| {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body = serde_json::json!({ "error": format!("forced status {code}") }).to_string();
            (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
        })
}

async fn parttable(State(s): State<Arc<AppState>>, Path(_task_id): Path<i64>) -> Response {
    if let Some(r) = forced(&s, Route::Parttable) {
        return r;
    }
    (
        [(header::CONTENT_TYPE, "application/octet-stream")],
        s.cfg.parttable.clone(),
    )
        .into_response()
}

async fn partitions(State(s): State<Arc<AppState>>, Path(_task_id): Path<i64>) -> Response {
    if let Some(r) = forced(&s, Route::Partitions) {
        return r;
    }
    Json(ImageLayout {
        sector_size: 512,
        partitions: s.cfg.partitions.clone(),
    })
    .into_response()
}

async fn partition_data(
    State(s): State<Arc<AppState>>,
    Path((_task_id, n)): Path<(i64, i64)>,
) -> Response {
    if let Some(r) = forced(&s, Route::PartitionData) {
        return r;
    }
    let Some(data) = s.cfg.partition_data.get(&n).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let data = Arc::new(data);
    let stream = futures::stream::unfold(0usize, move |offset| {
        let data = data.clone();
        async move {
            if offset >= data.len() {
                return None;
            }
            let end = (offset + CHUNK).min(data.len());
            let chunk = data[offset..end].to_vec();
            tokio::task::yield_now().await;
            Some((Ok::<Vec<u8>, std::io::Error>(chunk), end))
        }
    });

    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn post_install(State(s): State<Arc<AppState>>, Path(task_id): Path<i64>) -> Response {
    let plan = s
        .cfg
        .post_install
        .get(&task_id)
        .cloned()
        .unwrap_or(PostInstall {
            root_partition: None,
            proxy: None,
            variables: Default::default(),
            scripts: Vec::new(),
        });
    Json(plan).into_response()
}

async fn finished_route(State(s): State<Arc<AppState>>, Path(task_id): Path<i64>) -> Response {
    if let Some(r) = forced(&s, Route::Finished) {
        return r;
    }
    s.finished.lock().unwrap().push(task_id);
    StatusCode::OK.into_response()
}

async fn failed_route(
    State(s): State<Arc<AppState>>,
    Path(task_id): Path<i64>,
    body: Option<Json<TaskFailure>>,
) -> Response {
    if let Some(r) = forced(&s, Route::Failed) {
        return r;
    }
    let reason = body.map(|Json(b)| b.error).unwrap_or_default();
    s.failed.lock().unwrap().push((task_id, reason));
    StatusCode::OK.into_response()
}

async fn disconnect(State(s): State<Arc<AppState>>) -> Response {
    if let Some(r) = forced(&s, Route::Disconnect) {
        return r;
    }
    StatusCode::OK.into_response()
}

async fn stream(State(s): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    if let Some(r) = forced(&s, Route::Stream) {
        return r;
    }
    ws.on_upgrade(move |socket| handle_socket(s, socket))
}

fn event_frame(event: &ServerEvent) -> Message {
    let json = serde_json::to_string(event).expect("serialize server event");
    Message::Text(json.into())
}

async fn handle_socket(s: Arc<AppState>, mut socket: WebSocket) {
    // Subscribe before announcing the upgrade so a test that observes `upgrades()` is
    // guaranteed a live subscriber for any command it then pushes.
    let mut control = s.control.subscribe();
    s.upgrades.fetch_add(1, Ordering::SeqCst);

    for event in &s.cfg.events {
        if socket.send(event_frame(event)).await.is_err() {
            return;
        }
    }

    match s.cfg.stream_behavior {
        StreamBehavior::Close => {
            let _ = socket.send(Message::Close(None)).await;
        }
        StreamBehavior::Silent => {
            std::future::pending::<()>().await;
        }
        StreamBehavior::HoldOpen => loop {
            tokio::select! {
                command = control.recv() => match command {
                    Ok(StubCommand::Send(event)) => {
                        if socket.send(event_frame(&event)).await.is_err() {
                            break;
                        }
                    }
                    Ok(StubCommand::Ping) => {
                        if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                            break;
                        }
                    }
                    Ok(StubCommand::Close) => {
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Pong(_))) => {
                        s.pongs.fetch_add(1, Ordering::SeqCst);
                    }
                    Some(Ok(Message::Text(text))) => {
                        let event = serde_json::from_str(text.as_str()).expect("agent event");
                        s.agent_events.lock().unwrap().push(event);
                    }
                    Some(Ok(_)) => {}
                },
            }
        },
    }
}

pub fn agent_server(base_url: &str, mac: &str) -> Agent {
    let url = Url::parse(base_url).expect("valid base url");
    let parsed: MacAddress = mac.parse().expect("valid mac address");
    Agent {
        server: Arc::new(Server::new(url, parsed, None).expect("agent server client")),
        status: Status::new(Identity {
            mac: mac.to_string(),
            ip: None,
            server: base_url.to_string(),
        }),
    }
}

pub fn offline_agent() -> Agent {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe");
    let addr = listener.local_addr().expect("probe addr");
    drop(listener);
    agent_server(&format!("http://{addr}"), "aa:bb:cc:dd:ee:00")
}

pub struct FakeBins {
    dir: PathBuf,
    old_path: Option<OsString>,
}

pub fn fake_bins() -> FakeBins {
    FakeBins::new()
}

impl FakeBins {
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("imaged-fakebins-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create fake bin dir");

        let dir_str = dir.to_str().expect("utf8 fake bin dir").to_string();

        for name in ["sgdisk", "partprobe"] {
            let script =
                format!("#!/bin/sh\nif [ -f '{dir_str}/{name}.fail' ]; then exit 1; fi\nexit 0\n");
            Self::write_script(&dir.join(name), &script);
        }

        for name in ["partclone.extfs", "partclone.vfat"] {
            let script = format!(
                "#!/bin/sh\ncat > '{dir_str}/{name}.stdin'\nif [ -f '{dir_str}/{name}.fail' ]; then exit 1; fi\nexit 0\n"
            );
            Self::write_script(&dir.join(name), &script);
        }

        let old_path = std::env::var_os("PATH");
        let mut paths: Vec<PathBuf> = vec![dir.clone()];
        if let Some(existing) = &old_path {
            paths.extend(std::env::split_paths(existing));
        }
        let joined = std::env::join_paths(paths).expect("join PATH");
        // SAFETY: agent tests run on a current_thread runtime and must run sequentially
        // within a binary, so no other thread reads PATH concurrently.
        unsafe {
            std::env::set_var("PATH", &joined);
        }

        FakeBins { dir, old_path }
    }

    fn write_script(path: &std::path::Path, contents: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, contents).expect("write fake script");
        let mut perms = std::fs::metadata(path)
            .expect("stat fake script")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).expect("chmod fake script");
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn capture_path(&self, bin: &str) -> PathBuf {
        self.dir.join(format!("{bin}.stdin"))
    }

    pub fn captured(&self, bin: &str) -> Option<Vec<u8>> {
        std::fs::read(self.capture_path(bin)).ok()
    }

    pub fn fail(&self, bin: &str) {
        std::fs::write(self.dir.join(format!("{bin}.fail")), b"").expect("mark fail");
    }

    pub fn succeed(&self, bin: &str) {
        let _ = std::fs::remove_file(self.dir.join(format!("{bin}.fail")));
    }
}

impl Drop for FakeBins {
    fn drop(&mut self) {
        // SAFETY: see set_var in FakeBins::new; tests are sequential and single-threaded.
        unsafe {
            match &self.old_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
