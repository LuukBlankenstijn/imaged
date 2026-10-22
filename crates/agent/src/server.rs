use std::io;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use bytes::Bytes;
use futures::{SinkExt as _, Stream, StreamExt, TryStreamExt};
use imaged_model::agent::{
    IP_HEADER, ImageLayout, ImagePartition, MAC_HEADER, MacAddress, PartitionTableUpload,
    PartitionUpload, PostInstall, ScriptFinished, ScriptOutputUpload, StreamQuery, TaskFailure,
    path, route,
};
use imaged_model::progress::HostProgress;
use imaged_model::{AgentEvent, ServerEvent};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Body, Client, Method, RequestBuilder, Response, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::io::AsyncRead;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time::{Instant, Interval, MissedTickBehavior};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::io::{ReaderStream, StreamReader};

const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const KEEPALIVE_IDLE: Duration = Duration::from_secs(10);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
const KEEPALIVE_PROBES: u32 = 3;
const UNACKNOWLEDGED_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Server {
    http: Client,
    base: Url,
    headers: HeaderMap,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug)]
pub struct Rejected {
    request: String,
    status: StatusCode,
    message: String,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} failed with {}: {}",
            self.request, self.status, self.message
        )
    }
}

impl std::error::Error for Rejected {}

impl Rejected {
    pub fn is_final(error: &anyhow::Error) -> bool {
        error
            .downcast_ref::<Rejected>()
            .is_some_and(|rejected| rejected.status.is_client_error())
    }
}

pub struct Uplink {
    pub progress: watch::Receiver<Option<HostProgress>>,
    pub idle: watch::Receiver<()>,
}

impl Server {
    pub fn new(base: Url, mac: MacAddress, ip: Option<IpAddr>) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut headers = HeaderMap::new();
        headers.insert(MAC_HEADER, HeaderValue::from_str(&mac.to_string())?);
        if let Some(ip) = ip {
            headers.insert(IP_HEADER, HeaderValue::from_str(&ip.to_string())?);
        }
        let http = Client::builder()
            .default_headers(headers.clone())
            .connect_timeout(CONNECT_TIMEOUT)
            .tcp_keepalive(KEEPALIVE_IDLE)
            .tcp_keepalive_interval(KEEPALIVE_INTERVAL)
            .tcp_keepalive_retries(KEEPALIVE_PROBES)
            .tcp_user_timeout(UNACKNOWLEDGED_TIMEOUT)
            .build()
            .context("building the http client")?;
        Ok(Self {
            http,
            base,
            headers,
        })
    }

    pub fn base_url(&self) -> &str {
        self.base.as_str().trim_end_matches('/')
    }

    fn url(&self, path: &str) -> Result<Url> {
        let url = format!("{}{path}", self.base_url());
        Url::parse(&url).with_context(|| format!("invalid server url {url}"))
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        build: impl FnOnce(RequestBuilder) -> RequestBuilder,
    ) -> Result<Response> {
        let request = build(self.http.request(method.clone(), self.url(path)?));
        let response = request
            .send()
            .await
            .with_context(|| format!("{method} {path}"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<ErrorBody>(&body)
            .map(|body| body.error)
            .unwrap_or(body);
        Err(Rejected {
            request: format!("{method} {path}"),
            status,
            message,
        }
        .into())
    }

    async fn json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(Method::GET, path, |request| request)
            .await?
            .json()
            .await
            .with_context(|| format!("decoding the response of GET {path}"))
    }

    pub async fn connect_events(
        &self,
        disk: &std::result::Result<u64, String>,
        uplink: Uplink,
        silence_limit: Duration,
    ) -> Result<impl Stream<Item = ServerEvent> + Send + use<>> {
        let query = StreamQuery {
            disk_size_bytes: disk.as_ref().ok().copied(),
            disk_error: disk.as_ref().err().cloned(),
        };
        let url = self
            .http
            .get(self.url(route::STREAM)?)
            .query(&query)
            .build()?
            .url()
            .clone();
        let mut request = websocket_url(url)?.as_str().into_client_request()?;
        request.headers_mut().extend(self.headers.clone());
        let (socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .with_context(|| format!("GET {}", route::STREAM))?;
        let connection = Connection::new(socket, uplink, silence_limit);
        Ok(futures::stream::unfold(
            connection,
            |mut connection| async move {
                let event = connection.next_event().await?;
                Some((event, connection))
            },
        ))
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.send(Method::POST, route::DISCONNECT, |request| request)
            .await?;
        Ok(())
    }

    pub async fn mark_finished(&self, task_id: i64) -> Result<()> {
        self.send(Method::POST, &path::finished(task_id), |request| request)
            .await?;
        Ok(())
    }

    pub async fn mark_failed(&self, task_id: i64, error: &str) -> Result<()> {
        let body = TaskFailure {
            error: error.to_string(),
        };
        self.send(Method::POST, &path::failed(task_id), |request| {
            request.json(&body)
        })
        .await?;
        Ok(())
    }

    pub async fn image_layout(&self, task_id: i64) -> Result<ImageLayout> {
        self.json(&path::partitions(task_id)).await
    }

    pub async fn partition_table(&self, task_id: i64) -> Result<Bytes> {
        let path = path::partition_table(task_id);
        self.send(Method::GET, &path, |request| request)
            .await?
            .bytes()
            .await
            .with_context(|| format!("reading the response of GET {path}"))
    }

    pub async fn upload_partition_table(
        &self,
        task_id: i64,
        sector_size: u32,
        table: Bytes,
    ) -> Result<()> {
        self.send(Method::PUT, &path::partition_table(task_id), |request| {
            request
                .query(&PartitionTableUpload { sector_size })
                .body(table)
        })
        .await?;
        Ok(())
    }

    pub async fn partition_data(
        &self,
        task_id: i64,
        partition_number: i64,
    ) -> Result<impl AsyncRead + Unpin + Send> {
        let response = self
            .send(
                Method::GET,
                &path::partition_data(task_id, partition_number),
                |request| request,
            )
            .await?;
        Ok(StreamReader::new(
            response.bytes_stream().map_err(io::Error::other),
        ))
    }

    pub async fn upload_partition_data(
        &self,
        task_id: i64,
        partition_number: i64,
        filesystem: String,
        size: i64,
        data: impl AsyncRead + Send + 'static,
    ) -> Result<ImagePartition> {
        let path = path::partition_data(task_id, partition_number);
        self.send(Method::PUT, &path, |request| {
            request
                .query(&PartitionUpload { filesystem, size })
                .body(Body::wrap_stream(ReaderStream::new(data)))
        })
        .await?
        .json()
        .await
        .with_context(|| format!("decoding the response of PUT {path}"))
    }

    pub async fn report_root_partition(&self, task_id: i64, partition_number: i64) -> Result<()> {
        self.send(
            Method::PUT,
            &path::root_partition(task_id, partition_number),
            |request| request,
        )
        .await?;
        Ok(())
    }

    pub async fn post_install(&self, task_id: i64) -> Result<PostInstall> {
        self.json(&path::scripts(task_id)).await
    }

    pub async fn upload_script_output(
        &self,
        task_id: i64,
        position: i64,
        name: &str,
        output: impl Stream<Item = Bytes> + Send + 'static,
    ) -> Result<()> {
        self.send(
            Method::PUT,
            &path::script_output(task_id, position),
            |request| {
                request
                    .query(&ScriptOutputUpload {
                        name: name.to_owned(),
                    })
                    .body(Body::wrap_stream(output.map(Ok::<_, io::Error>)))
            },
        )
        .await?;
        Ok(())
    }

    pub async fn finish_script(
        &self,
        task_id: i64,
        position: i64,
        exit_code: Option<i64>,
    ) -> Result<()> {
        self.send(
            Method::POST,
            &path::script_finished(task_id, position),
            |request| request.json(&ScriptFinished { exit_code }),
        )
        .await?;
        Ok(())
    }
}

struct Connection {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    uplink: Uplink,
    interval: Interval,
    silence_limit: Duration,
    last_frame: Instant,
}

impl Connection {
    fn new(
        socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
        mut uplink: Uplink,
        silence_limit: Duration,
    ) -> Self {
        uplink.progress.mark_changed();
        uplink.idle.mark_unchanged();
        let mut interval = tokio::time::interval(PROGRESS_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            socket,
            uplink,
            interval,
            silence_limit,
            last_frame: Instant::now(),
        }
    }

    async fn next_event(&mut self) -> Option<ServerEvent> {
        loop {
            let frame = tokio::select! {
                frame = self.socket.next() => frame?,
                _ = self.interval.tick() => {
                    if let Err(e) = self.send_progress().await {
                        tracing::info!(err=%e, "could not report progress, closing the agent control stream");
                        return None;
                    }
                    continue;
                }
                Ok(()) = self.uplink.idle.changed() => {
                    if let Err(e) = self.send(&AgentEvent::Idle).await {
                        tracing::info!(err=%e, "could not ask for the next task, closing the agent control stream");
                        return None;
                    }
                    continue;
                }
                _ = tokio::time::sleep_until(self.last_frame + self.silence_limit) => {
                    tracing::warn!(silence=?self.silence_limit, "the server went silent, reconnecting");
                    return None;
                }
            };
            self.last_frame = Instant::now();
            let event = match frame {
                Ok(Message::Text(text)) => decode(text.as_bytes()),
                Ok(Message::Binary(bytes)) => decode(&bytes),
                Ok(Message::Close(_)) => return None,
                Ok(_) => continue,
                Err(e) => {
                    tracing::info!(err=%e, "agent control stream ended");
                    return None;
                }
            };
            if event.is_some() {
                return event;
            }
        }
    }

    async fn send_progress(&mut self) -> Result<(), tungstenite::Error> {
        if !self.uplink.progress.has_changed().unwrap_or(false) {
            return Ok(());
        }
        let Some(progress) = self.uplink.progress.borrow_and_update().clone() else {
            return Ok(());
        };
        tracing::debug!(task_id = progress.task_id, fraction = progress.fraction, stage = ?progress.stage, "reporting progress");
        self.send(&AgentEvent::from(progress)).await
    }

    async fn send(&mut self, event: &AgentEvent) -> Result<(), tungstenite::Error> {
        let json = serde_json::to_string(event).expect("agent events serialize to json");
        self.socket.send(Message::text(json)).await
    }
}

fn decode(payload: &[u8]) -> Option<ServerEvent> {
    match serde_json::from_slice(payload) {
        Ok(event) => Some(event),
        Err(e) => {
            tracing::warn!(err=%e, "ignoring an undecodable server event");
            None
        }
    }
}

fn websocket_url(mut url: Url) -> Result<Url> {
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        other => bail!("unsupported server url scheme {other}"),
    };
    if url.set_scheme(scheme).is_err() {
        bail!("could not turn {url} into a websocket url");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapped(url: &str) -> String {
        websocket_url(Url::parse(url).unwrap()).unwrap().to_string()
    }

    #[test]
    fn the_event_stream_uses_the_websocket_scheme_matching_the_server_scheme() {
        assert_eq!(
            mapped("http://10.0.2.2:8080/events?disk_size_bytes=1"),
            "ws://10.0.2.2:8080/events?disk_size_bytes=1"
        );
        assert_eq!(
            mapped("https://imaged.example/events"),
            "wss://imaged.example/events"
        );
        assert!(websocket_url(Url::parse("ftp://imaged.example/").unwrap()).is_err());
    }
}
