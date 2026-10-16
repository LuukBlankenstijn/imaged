use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::time::Duration;

use imaged_proxy::{Config, Proxy};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const LOCALHOST: &str = "127.0.0.1:0";
const DEADLINE: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(300);

async fn start(configure: impl FnOnce(&mut Config)) -> (SocketAddr, JoinHandle<()>) {
    let mut config = Config::new(LOCALHOST.parse().unwrap());
    config.allow_private_targets = true;
    configure(&mut config);
    let proxy = Proxy::bind(config).await.unwrap();
    let address = proxy.local_addr().unwrap();
    (address, tokio::spawn(proxy.serve()))
}

fn allowing(port: u16) -> impl FnOnce(&mut Config) {
    move |config| {
        config.allowed_connect_ports = BTreeSet::from([port]);
        config.allowed_forward_ports = BTreeSet::from([port]);
    }
}

async fn upstream_on(address: &str) -> (TcpListener, u16) {
    let listener = TcpListener::bind(address).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

async fn upstream() -> (TcpListener, u16) {
    upstream_on(LOCALHOST).await
}

async fn accept(listener: &TcpListener) -> TcpStream {
    timeout(DEADLINE, listener.accept())
        .await
        .unwrap()
        .unwrap()
        .0
}

async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(timeout(DEADLINE, stream.read_u8()).await.unwrap().unwrap());
    }
    String::from_utf8(head).unwrap()
}

async fn read_exact_string<S: AsyncRead + Unpin>(stream: &mut S, len: usize) -> String {
    let mut buf = vec![0; len];
    timeout(DEADLINE, stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(buf).unwrap()
}

async fn read_until_closed(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    timeout(DEADLINE, stream.read_to_end(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(buf).unwrap()
}

async fn assert_nothing_more(stream: &mut TcpStream) {
    let mut buf = [0; 64];
    if let Ok(read) = timeout(QUIET, stream.read(&mut buf)).await {
        let read = read.unwrap();
        panic!(
            "unexpected bytes: {:?}",
            String::from_utf8_lossy(&buf[..read])
        );
    }
}

async fn exchange(proxy: SocketAddr, request: &[u8]) -> String {
    let mut client = TcpStream::connect(proxy).await.unwrap();
    client.write_all(request).await.unwrap();
    read_until_closed(&mut client).await
}

async fn assert_no_connection(listener: &TcpListener) {
    assert!(timeout(QUIET, listener.accept()).await.is_err());
}

async fn open_tunnel(
    proxy: SocketAddr,
    listener: &TcpListener,
    port: u16,
) -> (TcpStream, TcpStream) {
    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(
        read_head(&mut client).await,
        "HTTP/1.1 200 Connection Established\r\n\r\n"
    );
    (client, accept(listener).await)
}

#[tokio::test]
async fn forwards_absolute_form_as_origin_form_without_proxy_headers() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;
    let response = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello";

    let origin = tokio::spawn(async move {
        let mut stream = accept(&listener).await;
        let head = read_head(&mut stream).await;
        stream.write_all(response.as_bytes()).await.unwrap();
        head
    });
    let reply = exchange(
        proxy,
        format!(
            "GET HTTP://127.0.0.1:{port}?q=1 HTTP/1.1\r\n\
             Host: wrong.example\r\n\
             Proxy-Connection: keep-alive\r\n\
             Proxy-Authorization: Basic Zm9vOmJhcg==\r\n\
             Connection: X-Hop, keep-alive\r\n\
             Keep-Alive: timeout=5\r\n\
             X-Hop: secret\r\n\
             Accept: */*\r\n\r\n"
        )
        .as_bytes(),
    )
    .await;

    assert_eq!(reply, response);
    assert_eq!(
        origin.await.unwrap(),
        format!(
            "GET /?q=1 HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Accept: */*\r\n\
             Connection: close\r\n\r\n"
        )
    );
}

#[tokio::test]
async fn forwards_only_the_first_request_on_a_connection() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;

    let origin = tokio::spawn(async move {
        let mut stream = accept(&listener).await;
        let head = read_head(&mut stream).await;
        assert_nothing_more(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
            .await
            .unwrap();
        head
    });
    let reply = exchange(
        proxy,
        format!(
            "GET http://127.0.0.1:{port}/ HTTP/1.1\r\n\r\n\
             GET http://other.example/secret HTTP/1.1\r\n\
             Authorization: Bearer TOKEN\r\n\r\n"
        )
        .as_bytes(),
    )
    .await;

    assert_eq!(reply, "HTTP/1.1 204 No Content\r\n\r\n");
    assert!(origin.await.unwrap().starts_with("GET / HTTP/1.1\r\n"));
}

#[tokio::test]
async fn forwards_exactly_the_content_length_body() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;

    let origin = tokio::spawn(async move {
        let mut stream = accept(&listener).await;
        let head = read_head(&mut stream).await;
        let body = read_exact_string(&mut stream, 11).await;
        assert_nothing_more(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
            .await
            .unwrap();
        (head, body)
    });
    let reply = exchange(
        proxy,
        format!(
            "POST http://127.0.0.1:{port} HTTP/1.1\r\nContent-Length: 11\r\n\r\nhello worldMAIL FROM:<x@y>\r\n"
        )
        .as_bytes(),
    )
    .await;

    assert_eq!(reply, "HTTP/1.1 204 No Content\r\n\r\n");
    let (head, body) = origin.await.unwrap();
    assert_eq!(
        head,
        format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 11\r\nConnection: close\r\n\r\n"
        )
    );
    assert_eq!(body, "hello world");
}

#[tokio::test]
async fn forwards_exactly_the_chunked_body_across_writes() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;
    let body = "5;ext=1\r\nhello\r\n6\r\n world\r\n0\r\nX-Trailer: 1\r\n\r\n";

    let origin = tokio::spawn(async move {
        let mut stream = accept(&listener).await;
        read_head(&mut stream).await;
        let received = read_exact_string(&mut stream, body.len()).await;
        assert_nothing_more(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
            .await
            .unwrap();
        received
    });
    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(
            format!(
                "POST http://127.0.0.1:{port}/ HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5;ext"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    sleep(Duration::from_millis(50)).await;
    client.write_all(&body.as_bytes()[5..]).await.unwrap();
    client
        .write_all(b"GET /next HTTP/1.1\r\n\r\n")
        .await
        .unwrap();

    assert_eq!(
        read_until_closed(&mut client).await,
        "HTTP/1.1 204 No Content\r\n\r\n"
    );
    assert_eq!(origin.await.unwrap(), body);
}

#[tokio::test]
async fn relays_interim_responses_before_the_body_is_sent() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;

    let origin = tokio::spawn(async move {
        let mut stream = accept(&listener).await;
        read_head(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .await
            .unwrap();
        let body = read_exact_string(&mut stream, 4).await;
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
            .await
            .unwrap();
        body
    });
    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(
            format!(
                "PUT http://127.0.0.1:{port}/ HTTP/1.1\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert_eq!(
        read_head(&mut client).await,
        "HTTP/1.1 100 Continue\r\n\r\n"
    );
    client.write_all(b"data").await.unwrap();

    assert_eq!(
        read_until_closed(&mut client).await,
        "HTTP/1.1 204 No Content\r\n\r\n"
    );
    assert_eq!(origin.await.unwrap(), "data");
}

#[tokio::test]
async fn upstream_closing_without_response_is_bad_gateway() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;

    let origin = tokio::spawn(async move {
        let mut stream = accept(&listener).await;
        read_head(&mut stream).await;
    });
    let reply = exchange(
        proxy,
        format!("GET http://127.0.0.1:{port}/ HTTP/1.1\r\n\r\n").as_bytes(),
    )
    .await;

    origin.await.unwrap();
    assert!(reply.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{reply}");
}

#[tokio::test]
async fn connect_to_allowed_port_tunnels_both_ways() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;

    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(
            format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\nearly")
                .as_bytes(),
        )
        .await
        .unwrap();
    assert_eq!(
        read_head(&mut client).await,
        "HTTP/1.1 200 Connection Established\r\n\r\n"
    );
    let mut origin = accept(&listener).await;
    assert_eq!(read_exact_string(&mut origin, 5).await, "early");

    client.write_all(b"ping").await.unwrap();
    assert_eq!(read_exact_string(&mut origin, 4).await, "ping");
    origin.write_all(b"pong").await.unwrap();
    assert_eq!(read_exact_string(&mut client, 4).await, "pong");

    origin.shutdown().await.unwrap();
    assert_eq!(read_until_closed(&mut client).await, "");
}

#[tokio::test]
async fn connect_to_bracketed_ipv6_target_tunnels() {
    let (listener, port) = upstream_on("[::1]:0").await;
    let (proxy, _server) = start(allowing(port)).await;

    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(format!("CONNECT [::1]:{port} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    read_head(&mut client).await;
    let mut origin = accept(&listener).await;
    client.write_all(b"ping").await.unwrap();

    assert_eq!(read_exact_string(&mut origin, 4).await, "ping");
}

#[tokio::test]
async fn disallowed_ports_are_forbidden_without_contacting_upstream() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(|_| {}).await;

    for request in [
        format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n"),
        format!("GET http://127.0.0.1:{port}/ HTTP/1.1\r\n\r\n"),
    ] {
        let reply = exchange(proxy, request.as_bytes()).await;
        assert!(reply.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{reply}");
    }
    assert_no_connection(&listener).await;
}

#[tokio::test]
async fn private_targets_are_forbidden_by_default() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(|config| {
        allowing(port)(config);
        config.allow_private_targets = false;
    })
    .await;

    for target in [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("2130706433:{port}"),
        format!("[::ffff:127.0.0.1]:{port}"),
        format!("[::1]:{port}"),
        format!("10.1.2.3:{port}"),
        format!("192.168.213.50:{port}"),
        format!("169.254.1.1:{port}"),
        format!("100.64.0.1:{port}"),
        format!("0.0.0.0:{port}"),
        format!("[fd00::1]:{port}"),
        format!("[fe80::1]:{port}"),
    ] {
        for request in [
            format!("CONNECT {target} HTTP/1.1\r\n\r\n"),
            format!("GET http://{target}/ HTTP/1.1\r\n\r\n"),
        ] {
            let reply = exchange(proxy, request.as_bytes()).await;
            assert!(
                reply.starts_with("HTTP/1.1 403 Forbidden\r\n"),
                "{request:?} -> {reply}"
            );
        }
    }
    assert_no_connection(&listener).await;
}

#[tokio::test]
async fn public_targets_pass_the_address_filter() {
    let documentation_address = "192.0.2.1:9";
    let (proxy, _server) = start(|config| {
        allowing(9)(config);
        config.allow_private_targets = false;
        config.connect_timeout = Duration::from_millis(200);
    })
    .await;

    let reply = exchange(
        proxy,
        format!("CONNECT {documentation_address} HTTP/1.1\r\n\r\n").as_bytes(),
    )
    .await;

    assert!(
        reply.starts_with("HTTP/1.1 504 Gateway Timeout\r\n")
            || reply.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{reply}"
    );
}

#[tokio::test]
async fn unreachable_upstream_is_bad_gateway() {
    let (closed, port) = upstream().await;
    drop(closed);
    let (proxy, _server) = start(allowing(port)).await;

    for request in [
        format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n"),
        format!("GET http://127.0.0.1:{port}/ HTTP/1.1\r\n\r\n"),
        format!("GET http://unresolvable.invalid:{port}/ HTTP/1.1\r\n\r\n"),
    ] {
        let reply = exchange(proxy, request.as_bytes()).await;
        assert!(reply.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{reply}");
    }
}

#[tokio::test]
async fn malformed_requests_are_bad_requests() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;
    let get = format!("GET http://127.0.0.1:{port}/ HTTP/1.1");
    let post = format!("POST http://127.0.0.1:{port}/ HTTP/1.1");

    for request in [
        "NOT A REQUEST\r\n\r\n".to_owned(),
        "GET /origin-form HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_owned(),
        format!("GET https://127.0.0.1:{port}/ HTTP/1.1\r\n\r\n"),
        "CONNECT 127.0.0.1 HTTP/1.1\r\n\r\n".to_owned(),
        "CONNECT 127.0.0.1:0 HTTP/1.1\r\n\r\n".to_owned(),
        "CONNECT 127.0.0.1:65536 HTTP/1.1\r\n\r\n".to_owned(),
        format!("CONNECT 127.0.0.1:+{port} HTTP/1.1\r\n\r\n"),
        format!("CONNECT a:b:{port} HTTP/1.1\r\n\r\n"),
        format!("CONNECT [127.0.0.1]:{port} HTTP/1.1\r\n\r\n"),
        format!("GET http://user@127.0.0.1:{port}/ HTTP/1.1\r\n\r\n"),
        format!("{get}\r\nBroken Header\r\n\r\n"),
        format!("{post}\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n"),
        format!("{post}\r\nContent-Length: 4\r\nContent-Length: 5\r\n\r\n"),
        format!("{post}\r\nContent-Length: -4\r\n\r\n"),
        format!("{post}\r\nTransfer-Encoding: chunked, gzip\r\n\r\n"),
        format!("{post}\r\nContent-Length: 4\r\nConnection: content-length\r\n\r\n"),
    ] {
        let reply = exchange(proxy, request.as_bytes()).await;
        assert!(
            reply.starts_with("HTTP/1.1 400 Bad Request\r\n"),
            "{request:?} -> {reply}"
        );
    }
    assert_no_connection(&listener).await;
}

#[tokio::test]
async fn oversized_headers_are_rejected() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(allowing(port)).await;

    let long_header = format!(
        "GET http://127.0.0.1:{port}/ HTTP/1.1\r\nX-Filler: {}\r\n\r\n",
        "a".repeat(20 * 1024)
    );
    let many_headers = format!(
        "GET http://127.0.0.1:{port}/ HTTP/1.1\r\n{}\r\n",
        "X-A: b\r\n".repeat(200)
    );
    for request in [long_header, many_headers] {
        let reply = exchange(proxy, request.as_bytes()).await;
        assert!(
            reply.starts_with("HTTP/1.1 431 Request Header Fields Too Large\r\n"),
            "{reply}"
        );
    }
    assert_no_connection(&listener).await;
}

#[tokio::test]
async fn trickling_request_head_times_out() {
    let (proxy, _server) = start(|config| {
        config.head_timeout = Duration::from_millis(300);
        config.idle_timeout = None;
    })
    .await;

    let mut client = TcpStream::connect(proxy).await.unwrap();
    let trickle = async {
        for byte in b"GET http://example.com/ HTTP/1.1\r\n".iter().cycle() {
            client.write_all(&[*byte]).await?;
            sleep(Duration::from_millis(20)).await;
        }
        Ok::<_, std::io::Error>(())
    };
    let _ = timeout(Duration::from_secs(1), trickle).await;
    let reply = read_until_closed(&mut client).await;

    assert!(
        reply.starts_with("HTTP/1.1 408 Request Timeout\r\n"),
        "{reply}"
    );
}

#[tokio::test]
async fn idle_tunnel_is_closed() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(|config| {
        allowing(port)(config);
        config.idle_timeout = Some(Duration::from_millis(200));
    })
    .await;

    let (mut client, mut origin) = open_tunnel(proxy, &listener, port).await;

    assert_eq!(read_until_closed(&mut client).await, "");
    assert_eq!(read_until_closed(&mut origin).await, "");
}

#[tokio::test]
async fn connections_beyond_the_limit_are_refused_until_a_slot_frees() {
    let (listener, port) = upstream().await;
    let (proxy, _server) = start(|config| {
        allowing(port)(config);
        config.max_connections = 1;
    })
    .await;

    let (client, origin) = open_tunnel(proxy, &listener, port).await;
    let mut refused = TcpStream::connect(proxy).await.unwrap();
    let reply = read_until_closed(&mut refused).await;
    assert!(
        reply.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "{reply}"
    );

    drop((client, origin));
    timeout(DEADLINE, async {
        loop {
            let mut client = TcpStream::connect(proxy).await.unwrap();
            client
                .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes())
                .await
                .unwrap();
            if read_head(&mut client).await.starts_with("HTTP/1.1 200 ") {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn stopping_the_server_closes_open_tunnels() {
    let (listener, port) = upstream().await;
    let (proxy, server) = start(|config| {
        allowing(port)(config);
        config.idle_timeout = None;
    })
    .await;

    let (mut client, mut origin) = open_tunnel(proxy, &listener, port).await;

    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());

    assert_eq!(read_until_closed(&mut client).await, "");
    assert_eq!(read_until_closed(&mut origin).await, "");
    assert!(TcpStream::connect(proxy).await.is_err());
}
