mod relay;
mod request;
mod status;
mod target;

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, lookup_host};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tracing::field::{Empty, display};
use tracing::{Instrument, Span, error, info, info_span, warn};

use relay::{Direction, Metered, Traffic};
use request::{Kind, MAX_HEAD_LEN, Request, Target};
use status::Status;

const DEFAULT_CONNECT_PORTS: [u16; 2] = [443, 22];
const DEFAULT_FORWARD_PORTS: [u16; 1] = [80];
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_HEAD_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_MAX_CONNECTIONS: usize = 64;
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
const CLOSE_LINGER: Duration = Duration::from_secs(2);
const CONNECTION_ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Config {
    pub bind: SocketAddr,
    pub allowed_connect_ports: BTreeSet<u16>,
    pub allowed_forward_ports: BTreeSet<u16>,
    pub allow_private_targets: bool,
    pub connect_timeout: Duration,
    pub head_timeout: Duration,
    pub idle_timeout: Option<Duration>,
    pub max_connections: usize,
}

impl Config {
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            allowed_connect_ports: BTreeSet::from(DEFAULT_CONNECT_PORTS),
            allowed_forward_ports: BTreeSet::from(DEFAULT_FORWARD_PORTS),
            allow_private_targets: false,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            head_timeout: DEFAULT_HEAD_TIMEOUT,
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }

    fn allows_port(&self, kind: &Kind, port: u16) -> bool {
        match kind {
            Kind::Connect => self.allowed_connect_ports.contains(&port),
            Kind::Forward { .. } => self.allowed_forward_ports.contains(&port),
        }
    }

    fn allows_address(&self, address: SocketAddr, own_addresses: &[IpAddr]) -> bool {
        let ip = target::canonical(address.ip());
        self.allow_private_targets || (target::is_public(ip) && !own_addresses.contains(&ip))
    }
}

fn own_addresses() -> io::Result<Vec<IpAddr>> {
    let interfaces = local_ip_address::list_afinet_netifas().map_err(io::Error::other)?;
    Ok(interfaces
        .into_iter()
        .map(|(_, ip)| target::canonical(ip))
        .collect())
}

pub struct Proxy {
    listener: TcpListener,
    config: Arc<Config>,
}

impl Proxy {
    pub async fn bind(config: Config) -> io::Result<Self> {
        let listener = TcpListener::bind(config.bind).await?;
        Ok(Self {
            listener,
            config: Arc::new(config),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn serve(self) {
        let mut connections = JoinSet::new();
        let slots = Arc::new(Semaphore::new(self.config.max_connections));
        loop {
            tokio::select! {
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                            warn!(client = %peer, "refusing proxy connection: too many connections");
                            refuse(stream);
                            continue;
                        };
                        let span = info_span!("proxy", client = %peer, method = Empty, target = Empty);
                        let connection = handle(stream, Arc::clone(&self.config)).instrument(span);
                        connections.spawn(async move {
                            connection.await;
                            drop(slot);
                        });
                    }
                    Err(error) => {
                        warn!(%error, "accepting proxy connection failed");
                        sleep(ACCEPT_BACKOFF).await;
                    }
                },
                Some(joined) = connections.join_next() => {
                    if let Err(error) = joined {
                        error!(%error, "proxy connection task failed");
                    }
                }
            }
        }
    }
}

fn refuse(stream: TcpStream) {
    let response = Status::ServiceUnavailable.response();
    if let Err(error) = stream
        .into_std()
        .and_then(|mut stream| stream.write_all(response.as_bytes()))
    {
        warn!(%error, "sending service unavailable failed");
    }
}

enum Outcome {
    Relayed,
    Rejected(Status),
    Abandoned,
    Idle,
    Failed(io::Error),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relayed => f.write_str("relayed"),
            Self::Rejected(status) => write!(f, "rejected with {status}"),
            Self::Abandoned => f.write_str("client closed before completing a request"),
            Self::Idle => f.write_str("closed after idle timeout"),
            Self::Failed(error) => write!(f, "failed: {error}"),
        }
    }
}

enum Incoming {
    Request(Request, Vec<u8>),
    Rejected(Status),
    Closed,
}

async fn handle(client: TcpStream, config: Arc<Config>) {
    let started = Instant::now();
    let traffic = Traffic::new();
    let outcome = session(client, &config, &traffic)
        .await
        .unwrap_or_else(Outcome::Failed);
    info!(
        %outcome,
        up = traffic.up(),
        down = traffic.down(),
        duration = ?started.elapsed(),
        "proxy connection closed"
    );
}

async fn session(client: TcpStream, config: &Config, traffic: &Traffic) -> io::Result<Outcome> {
    client.set_nodelay(true)?;
    let mut client = Metered::new(client, traffic, Direction::Up);
    let incoming = timeout(config.head_timeout, read_request(&mut client))
        .await
        .unwrap_or(Ok(Incoming::Rejected(Status::RequestTimeout)))?;
    let (request, early_data) = match incoming {
        Incoming::Request(request, early_data) => (request, early_data),
        Incoming::Rejected(status) => return reject(client, status).await,
        Incoming::Closed => return Ok(Outcome::Abandoned),
    };
    let span = Span::current();
    span.record("method", request.method.as_str());
    span.record("target", display(&request.target));

    if !config.allows_port(&request.kind, request.target.port) {
        return reject(client, Status::Forbidden).await;
    }
    let upstream = match connect(&request.target, config).await {
        Ok(upstream) => upstream,
        Err(status) => return reject(client, status).await,
    };
    upstream.set_nodelay(true)?;
    let mut upstream = Metered::new(upstream, traffic, Direction::Down);
    match request.kind {
        Kind::Connect => {
            client.write_all(CONNECTION_ESTABLISHED).await?;
            upstream.write_all(&early_data).await?;
            tokio::select! {
                result = relay::tunnel(&mut client, &mut upstream) => result.map(|()| Outcome::Relayed),
                () = traffic.idle(config.idle_timeout) => Ok(Outcome::Idle),
            }
        }
        Kind::Forward { head, body } => {
            upstream.write_all(&head).await?;
            let relayed = tokio::select! {
                result = relay::forward(&mut client, &mut upstream, &early_data, body) => Some(result),
                () = traffic.idle(config.idle_timeout) => None,
            };
            match (traffic.down(), relayed) {
                (0, None) => reject(client, Status::GatewayTimeout).await,
                (0, Some(result)) => {
                    if let Err(error) = result {
                        warn!(%error, "forwarding request failed");
                    }
                    reject(client, Status::BadGateway).await
                }
                (_, None) => Ok(Outcome::Idle),
                (_, Some(result)) => {
                    result?;
                    close(client).await?;
                    Ok(Outcome::Relayed)
                }
            }
        }
    }
}

async fn read_request<S: AsyncRead + Unpin>(client: &mut S) -> io::Result<Incoming> {
    let mut buf = vec![0; MAX_HEAD_LEN];
    let mut filled = 0;
    loop {
        if filled == buf.len() {
            return Ok(Incoming::Rejected(Status::HeaderTooLarge));
        }
        let read = client.read(&mut buf[filled..]).await?;
        if read == 0 {
            return Ok(Incoming::Closed);
        }
        filled += read;
        match request::parse(&buf[..filled]) {
            Ok(Some((request, head_len))) => {
                return Ok(Incoming::Request(request, buf[head_len..filled].to_vec()));
            }
            Ok(None) => {}
            Err(status) => return Ok(Incoming::Rejected(status)),
        }
    }
}

async fn reject<S: AsyncRead + AsyncWrite + Unpin>(
    mut client: S,
    status: Status,
) -> io::Result<Outcome> {
    client.write_all(status.response().as_bytes()).await?;
    close(client).await?;
    Ok(Outcome::Rejected(status))
}

async fn close<S: AsyncRead + AsyncWrite + Unpin>(mut client: S) -> io::Result<()> {
    client.shutdown().await?;
    let _ = timeout(
        CLOSE_LINGER,
        tokio::io::copy(&mut client, &mut tokio::io::sink()),
    )
    .await;
    Ok(())
}

async fn connect(target: &Target, config: &Config) -> Result<TcpStream, Status> {
    let resolved = timeout(
        config.connect_timeout,
        lookup_host((target.host.as_str(), target.port)),
    )
    .await;
    let addresses: Vec<SocketAddr> = match resolved {
        Ok(Ok(addresses)) => addresses.collect(),
        Ok(Err(error)) => {
            warn!(%error, "resolving upstream failed");
            return Err(Status::BadGateway);
        }
        Err(_) => return Err(Status::GatewayTimeout),
    };
    let own_addresses = if config.allow_private_targets {
        Vec::new()
    } else {
        own_addresses().map_err(|error| {
            warn!(%error, "listing the server's own addresses failed");
            Status::BadGateway
        })?
    };
    let allowed: Vec<SocketAddr> = addresses
        .iter()
        .copied()
        .filter(|&address| config.allows_address(address, &own_addresses))
        .collect();
    if allowed.is_empty() && !addresses.is_empty() {
        warn!(
            ?addresses,
            "upstream resolves only to private or own addresses"
        );
        return Err(Status::Forbidden);
    }
    let mut status = Status::BadGateway;
    for address in allowed {
        match timeout(config.connect_timeout, TcpStream::connect(address)).await {
            Ok(Ok(upstream)) => return Ok(upstream),
            Ok(Err(error)) => {
                warn!(%error, %address, "upstream connect failed");
                status = Status::BadGateway;
            }
            Err(_) => {
                warn!(%address, "upstream connect timed out");
                status = Status::GatewayTimeout;
            }
        }
    }
    Err(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_own_public_addresses_are_refused_in_every_form() {
        let config = Config::new("0.0.0.0:3128".parse().unwrap());
        let own = ["203.0.113.7".parse().unwrap()];
        for target in [
            "203.0.113.7:443",
            "[::ffff:203.0.113.7]:443",
            "[64:ff9b::cb00:7107]:443",
        ] {
            assert!(
                !config.allows_address(target.parse().unwrap(), &own),
                "{target}"
            );
        }
        assert!(config.allows_address("203.0.113.8:443".parse().unwrap(), &own));
    }

    #[test]
    fn allowing_private_targets_also_allows_the_servers_own_addresses() {
        let mut config = Config::new("127.0.0.1:3128".parse().unwrap());
        config.allow_private_targets = true;
        let own = ["203.0.113.7".parse().unwrap()];
        assert!(config.allows_address("203.0.113.7:443".parse().unwrap(), &own));
    }
}
