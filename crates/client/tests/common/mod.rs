#![allow(dead_code)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde::Deserialize;
use tokio::net::TcpListener;

use api::model::ImagePartition;
use imaged_shared::ServerEvent;

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

#[derive(Default, Clone)]
pub struct StubConfig {
    pub parttable: Vec<u8>,
    pub partitions: Vec<ImagePartition>,
    pub partition_data: HashMap<i64, Vec<u8>>,
    pub events: Vec<ServerEvent>,
    pub force_status: HashMap<Route, u16>,
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

    pub fn force(mut self, route: Route, status: u16) -> Self {
        self.force_status.insert(route, status);
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
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    finished: Arc<Mutex<Vec<i64>>>,
    failed: Arc<Mutex<Vec<(i64, String)>>>,
}

pub struct StubServer {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
    pub finished: Arc<Mutex<Vec<i64>>>,
    pub failed: Arc<Mutex<Vec<(i64, String)>>>,
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

    let state = Arc::new(AppState {
        cfg,
        requests: requests.clone(),
        finished: finished.clone(),
        failed: failed.clone(),
    });

    let app = Router::new()
        .route("/api/client/tasks/{task_id}/parttable", get(parttable))
        .route("/api/client/tasks/{task_id}/partitions", get(partitions))
        .route(
            "/api/client/tasks/{task_id}/partitions/{n}/data",
            get(partition_data),
        )
        .route("/api/client/tasks/{task_id}/finished", post(finished_route))
        .route("/api/client/tasks/{task_id}/failed", post(failed_route))
        .route("/api/client/stream/disconnect", post(disconnect))
        .route("/api/client/stream", get(stream))
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
        handle,
    }
}

async fn record_mw(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let mac = req
        .headers()
        .get("X-Agent-Mac")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    state
        .requests
        .lock()
        .unwrap()
        .push(RecordedRequest { method, path, mac });
    next.run(req).await
}

fn forced(cfg: &StubConfig, route: Route) -> Option<Response> {
    cfg.force_status.get(&route).map(|&code| {
        let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::json!({
            "message": format!("forced status {code}"),
            "code": code,
        })
        .to_string();
        (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
    })
}

async fn parttable(State(s): State<Arc<AppState>>, Path(_task_id): Path<i64>) -> Response {
    if let Some(r) = forced(&s.cfg, Route::Parttable) {
        return r;
    }
    Json(s.cfg.parttable.clone()).into_response()
}

async fn partitions(State(s): State<Arc<AppState>>, Path(_task_id): Path<i64>) -> Response {
    if let Some(r) = forced(&s.cfg, Route::Partitions) {
        return r;
    }
    Json(s.cfg.partitions.clone()).into_response()
}

async fn partition_data(
    State(s): State<Arc<AppState>>,
    Path((_task_id, n)): Path<(i64, i64)>,
) -> Response {
    if let Some(r) = forced(&s.cfg, Route::PartitionData) {
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

async fn finished_route(State(s): State<Arc<AppState>>, Path(task_id): Path<i64>) -> Response {
    if let Some(r) = forced(&s.cfg, Route::Finished) {
        return r;
    }
    s.finished.lock().unwrap().push(task_id);
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
struct FailedBody {
    error: String,
}

async fn failed_route(
    State(s): State<Arc<AppState>>,
    Path(task_id): Path<i64>,
    body: Option<Json<FailedBody>>,
) -> Response {
    if let Some(r) = forced(&s.cfg, Route::Failed) {
        return r;
    }
    let reason = body.map(|Json(b)| b.error).unwrap_or_default();
    s.failed.lock().unwrap().push((task_id, reason));
    StatusCode::OK.into_response()
}

async fn disconnect(State(s): State<Arc<AppState>>) -> Response {
    if let Some(r) = forced(&s.cfg, Route::Disconnect) {
        return r;
    }
    StatusCode::OK.into_response()
}

async fn stream(State(s): State<Arc<AppState>>) -> Response {
    if let Some(r) = forced(&s.cfg, Route::Stream) {
        return r;
    }
    let frames: Vec<Result<Vec<u8>, std::io::Error>> = s
        .cfg
        .events
        .iter()
        .map(|ev| {
            let json = serde_json::to_string(ev).unwrap();
            Ok(format!("data: {json}\n\n").into_bytes())
        })
        .collect();
    let body_stream = futures::stream::iter(frames).then(|frame| async move {
        tokio::task::yield_now().await;
        frame
    });

    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(body_stream))
        .unwrap()
}

static INIT_TRANSPORT: Once = Once::new();

pub fn init_transport(base_url: &str, mac: &str) {
    INIT_TRANSPORT.call_once(|| {
        let url = dioxus_fullstack::reqwest::Url::parse(base_url).expect("valid base url");
        let mac: mac_address::MacAddress = mac.parse().expect("valid mac address");
        imaged_client::transport::setup_transport(url, mac, None).expect("setup transport");
    });
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
        let dir = std::env::temp_dir().join(format!("imaged-fakebins-{}-{nanos}", std::process::id()));
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
