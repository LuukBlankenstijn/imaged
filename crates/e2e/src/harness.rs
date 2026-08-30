use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use imaged_core::di::DIContainer;

// Server functions self-register via `inventory` at load time, which only
// happens for crates actually linked into the binary.
use imaged_api_client as _;
use imaged_api_ui as _;

pub mod seed;

pub struct TestServer {
    pub base_url: String,
    pub container: DIContainer,
    pub images_dir: PathBuf,
    pub http: reqwest::Client,
}

static SERVER: LazyLock<TestServer> = LazyLock::new(|| {
    std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build harness runtime");
        let server = runtime.block_on(build_server());
        // Keep the runtime — and thus the spawned axum server — alive for the
        // whole process.
        std::mem::forget(runtime);
        server
    })
    .join()
    .expect("harness init thread panicked")
});

/// Process-wide singleton HTTP server backed by a fresh migrated SQLite DB.
///
/// The server runs on its own dedicated multi-threaded runtime so it outlives
/// the per-test `#[tokio::test]` runtimes that call this — every test in the
/// binary shares the same instance (and therefore the same database).
pub async fn server() -> &'static TestServer {
    LazyLock::force(&SERVER)
}

async fn build_server() -> TestServer {
    let dir = std::env::temp_dir().join(format!("imaged-e2e-{}", std::process::id()));
    let container = imaged_core::build_test_container(&dir).await;
    imaged_core::di::init_container(container.clone());

    let router = collect_routes(|path| path.starts_with("/api/client"))
        .merge(collect_routes(|path| !path.starts_with("/api/client")))
        .with_state(dioxus_server::FullstackState::headless());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind harness listener");
    let addr = listener.local_addr().expect("harness local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service()).await;
    });

    TestServer {
        base_url: format!("http://{addr}"),
        container,
        images_dir: dir.join("images"),
        // No idle pool: the client is a `&'static` shared across every per-test
        // `#[tokio::test]` runtime. A pooled idle connection outlives the test
        // that created it; when that runtime is dropped the connection's
        // dispatch task dies and the next test reusing it panics with
        // hyper `DispatchGone`. Fresh connection per request on the caller's
        // own runtime.
        http: reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()
            .expect("build harness http client"),
    }
}

fn collect_routes(keep: impl Fn(&str) -> bool) -> axum::Router<dioxus_server::FullstackState> {
    dioxus_server::ServerFunction::collect()
        .into_iter()
        .filter(|f| keep(f.path()))
        .fold(axum::Router::new(), |router, f| {
            router.route(f.path(), f.method_router())
        })
}

static MAC_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A locally-administered unicast MAC unique within this process.
pub fn unique_mac() -> String {
    let n = MAC_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "02:00:00:{:02x}:{:02x}:{:02x}",
        (n >> 16) & 0xff,
        (n >> 8) & 0xff,
        n & 0xff,
    )
}

static NAME_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A name unique within this process, prefixed for readability.
pub fn unique_name(prefix: &str) -> String {
    let n = NAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{}", std::process::id(), n)
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub async fn agent_get(&self, path: &str, mac: &str) -> reqwest::Response {
        self.http
            .get(self.url(path))
            .header("X-Agent-Mac", mac)
            .send()
            .await
            .expect("agent_get send")
    }

    pub async fn agent_post(&self, path: &str, mac: &str, body: Bytes) -> reqwest::Response {
        self.http
            .post(self.url(path))
            .header("X-Agent-Mac", mac)
            .body(body)
            .send()
            .await
            .expect("agent_post send")
    }

    pub async fn agent_post_empty(&self, path: &str, mac: &str) -> reqwest::Response {
        self.http
            .post(self.url(path))
            .header("X-Agent-Mac", mac)
            .send()
            .await
            .expect("agent_post_empty send")
    }

    /// PUT a `Bytes` server-fn argument. Such an argument is `Serialize`, so it
    /// does not travel as a raw octet-stream body the way a `ByteStream`
    /// argument does: it is JSON, in the same argument-name envelope every other
    /// non-path argument uses.
    pub async fn agent_put_bytes_arg(
        &self,
        path: &str,
        mac: &str,
        arg: &str,
        body: &[u8],
    ) -> reqwest::Response {
        self.http
            .put(self.url(path))
            .header("X-Agent-Mac", mac)
            .json(&serde_json::json!({ arg: body }))
            .send()
            .await
            .expect("agent_put_bytes_arg send")
    }

    /// PUT an `application/octet-stream` request whose body is streamed chunk by
    /// chunk — the shape the agent's capture upload sends.
    pub async fn agent_put_stream<S>(&self, path: &str, mac: &str, body: S) -> reqwest::Response
    where
        S: Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send + 'static,
    {
        self.http
            .put(self.url(path))
            .header("X-Agent-Mac", mac)
            .header("Content-Type", "application/octet-stream")
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await
            .expect("agent_put_stream send")
    }

    pub async fn ui_get(&self, path: &str) -> reqwest::Response {
        self.http
            .get(self.url(path))
            .send()
            .await
            .expect("ui_get send")
    }

    pub async fn ui_post<T: serde::Serialize>(&self, path: &str, body: &T) -> reqwest::Response {
        self.http
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .expect("ui_post send")
    }
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

enum Inner {
    Sse {
        stream: BoxStream<'static, reqwest::Result<Bytes>>,
        buf: Vec<u8>,
    },
    Ws(WsStream),
}

/// A live agent control connection (a WebSocket) or dashboard SSE feed.
///
/// Dropping it closes the underlying TCP connection: for the WebSocket that
/// sends a FIN the server observes on its concurrent read, driving the
/// server-side `Registration` cleanup promptly.
pub struct EventStream {
    inner: Inner,
}

pub async fn connect_agent_stream(
    s: &TestServer,
    mac: &str,
    disk_size_bytes: u64,
) -> anyhow::Result<EventStream> {
    let ws_base = s.base_url.replacen("http://", "ws://", 1);
    let url = format!("{ws_base}/api/client/stream?disk_size_bytes={disk_size_bytes}");
    let mut request = url.into_client_request()?;
    request.headers_mut().insert("X-Agent-Mac", mac.parse()?);
    let (ws, _resp) = connect_async(request).await?;
    Ok(EventStream {
        inner: Inner::Ws(ws),
    })
}

pub async fn connect_connection_state(s: &TestServer) -> anyhow::Result<EventStream> {
    let url = format!("{}/api/ui/connection-state", s.base_url);
    let resp = s.http.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("connection-state request failed with status {status}");
    }
    Ok(EventStream {
        inner: Inner::Sse {
            stream: resp.bytes_stream().boxed(),
            buf: Vec::new(),
        },
    })
}

impl EventStream {
    /// Return the next event's JSON payload, or `None` on timeout or end of
    /// stream. For the WebSocket that is the next data frame's JSON — dioxus's
    /// default `JsonEncoding` frames typed messages as Binary, so both Binary
    /// and Text are decoded — with Ping/Pong control traffic transparently
    /// ignored; for SSE it is the next event's `data` payload, buffered across
    /// chunk boundaries.
    pub async fn next_event(&mut self, timeout: Duration) -> Option<serde_json::Value> {
        let deadline = Instant::now() + timeout;
        loop {
            match &mut self.inner {
                Inner::Sse { stream, buf } => {
                    if let Some(event) = take_buffered_event(buf) {
                        return Some(event);
                    }
                    let remaining = deadline.checked_duration_since(Instant::now())?;
                    match tokio::time::timeout(remaining, stream.next()).await {
                        Ok(Some(Ok(chunk))) => {
                            buf.extend(chunk.iter().copied().filter(|&b| b != b'\r'));
                        }
                        Ok(Some(Err(_))) | Ok(None) | Err(_) => return None,
                    }
                }
                Inner::Ws(ws) => {
                    let remaining = deadline.checked_duration_since(Instant::now())?;
                    match tokio::time::timeout(remaining, ws.next()).await {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            if let Ok(value) = serde_json::from_str(text.as_str()) {
                                return Some(value);
                            }
                        }
                        Ok(Some(Ok(Message::Binary(bytes)))) => {
                            if let Ok(value) = serde_json::from_slice(&bytes) {
                                return Some(value);
                            }
                        }
                        Ok(Some(Ok(Message::Close(_)))) => return None,
                        Ok(Some(Ok(_))) => continue,
                        Ok(Some(Err(_))) | Ok(None) | Err(_) => return None,
                    }
                }
            }
        }
    }
}

fn take_buffered_event(buf: &mut Vec<u8>) -> Option<serde_json::Value> {
    loop {
        let boundary = buf.windows(2).position(|w| w == b"\n\n")?;
        let block = buf[..boundary].to_vec();
        buf.drain(..boundary + 2);

        let mut data = String::new();
        for line in block.split(|&b| b == b'\n') {
            if let Some(rest) = line.strip_prefix(b"data:") {
                let rest = rest.strip_prefix(b" ").unwrap_or(rest);
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(&String::from_utf8_lossy(rest));
            }
        }

        if data.is_empty() {
            continue;
        }
        return serde_json::from_str(&data).ok();
    }
}
