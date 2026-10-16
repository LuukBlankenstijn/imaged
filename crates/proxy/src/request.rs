use std::fmt;
use std::net::Ipv6Addr;
use std::str::FromStr;

use crate::status::Status;

pub(crate) const MAX_HEAD_LEN: usize = 16 * 1024;
const MAX_HEADERS: usize = 100;
const HTTP_SCHEME: &str = "http://";
const DEFAULT_HTTP_PORT: u16 = 80;
const CHUNKED: &str = "chunked";
const FRAMING_HEADERS: [&str; 2] = ["content-length", "transfer-encoding"];
const DROPPED_HEADERS: [&str; 9] = [
    "host",
    "connection",
    "proxy-connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "upgrade",
];

#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) target: Target,
    pub(crate) kind: Kind,
}

#[derive(Debug)]
pub(crate) enum Kind {
    Connect,
    Forward { head: Vec<u8>, body: Body },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Body {
    Length(u64),
    Chunked,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) host: String,
    pub(crate) port: u16,
}

impl Target {
    fn uri_host(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    fn host_header(&self) -> String {
        if self.port == DEFAULT_HTTP_PORT {
            self.uri_host()
        } else {
            self.to_string()
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.uri_host(), self.port)
    }
}

pub(crate) fn parse(buf: &[u8]) -> Result<Option<(Request, usize)>, Status> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut parsed = httparse::Request::new(&mut headers);
    let head_len = match parsed.parse(buf) {
        Ok(httparse::Status::Complete(len)) => len,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(httparse::Error::TooManyHeaders) => return Err(Status::HeaderTooLarge),
        Err(_) => return Err(Status::BadRequest),
    };
    let (Some(method), Some(uri), Some(version)) = (parsed.method, parsed.path, parsed.version)
    else {
        return Err(Status::BadRequest);
    };
    let request = if method == "CONNECT" {
        Request {
            method: method.to_owned(),
            target: parse_authority(uri, None).ok_or(Status::BadRequest)?,
            kind: Kind::Connect,
        }
    } else {
        let (authority, origin) = split_http_uri(uri).ok_or(Status::BadRequest)?;
        let target =
            parse_authority(authority, Some(DEFAULT_HTTP_PORT)).ok_or(Status::BadRequest)?;
        let body = body_framing(parsed.headers).ok_or(Status::BadRequest)?;
        let connection_listed = connection_listed(parsed.headers).ok_or(Status::BadRequest)?;
        let head = rewrite_head(
            method,
            &origin,
            version,
            &target,
            parsed.headers,
            &connection_listed,
        );
        Request {
            method: method.to_owned(),
            target,
            kind: Kind::Forward { head, body },
        }
    };
    Ok(Some((request, head_len)))
}

fn split_http_uri(uri: &str) -> Option<(&str, String)> {
    let scheme = uri.get(..HTTP_SCHEME.len())?;
    if !scheme.eq_ignore_ascii_case(HTTP_SCHEME) {
        return None;
    }
    let rest = &uri[HTTP_SCHEME.len()..];
    let (authority, path) = rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()));
    let origin = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    Some((authority, origin))
}

fn parse_authority(authority: &str, default_port: Option<u16>) -> Option<Target> {
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']')?;
            host.parse::<Ipv6Addr>().ok()?;
            let port = match after {
                "" => None,
                after => Some(after.strip_prefix(':')?),
            };
            (host, port)
        }
        None => {
            let (host, port) = match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            };
            let valid_host = !host.is_empty()
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b));
            valid_host.then_some((host, port))?
        }
    };
    let port = match port {
        Some(port) => parse_port(port)?,
        None => default_port?,
    };
    Some(Target {
        host: host.to_owned(),
        port,
    })
}

fn parse_port(port: &str) -> Option<u16> {
    parse_digits(port).filter(|&port| port != 0)
}

fn parse_digits<T: FromStr>(value: &str) -> Option<T> {
    let digits = !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
    digits.then(|| value.parse().ok()).flatten()
}

fn header_values<'a>(
    headers: &'a [httparse::Header<'a>],
    name: &'a str,
) -> impl Iterator<Item = Option<&'a str>> + 'a {
    headers
        .iter()
        .filter(move |header| header.name.eq_ignore_ascii_case(name))
        .map(|header| std::str::from_utf8(header.value).ok())
}

fn body_framing(headers: &[httparse::Header<'_>]) -> Option<Body> {
    let encodings = header_values(headers, "transfer-encoding")
        .map(|value| value.map(|value| value.split(',').map(str::trim)))
        .collect::<Option<Vec<_>>>()?;
    let final_encoding = encodings.into_iter().flatten().last();
    let lengths = header_values(headers, "content-length")
        .map(|value| value.and_then(parse_digits))
        .collect::<Option<Vec<_>>>()?;
    match (final_encoding, lengths.split_first()) {
        (Some(encoding), None) => encoding
            .eq_ignore_ascii_case(CHUNKED)
            .then_some(Body::Chunked),
        (Some(_), Some(_)) => None,
        (None, None) => Some(Body::Length(0)),
        (None, Some((&length, rest))) => rest
            .iter()
            .all(|&other| other == length)
            .then_some(Body::Length(length)),
    }
}

fn connection_listed<'a>(headers: &'a [httparse::Header<'a>]) -> Option<Vec<&'a str>> {
    let values = header_values(headers, "connection")
        .chain(header_values(headers, "proxy-connection"))
        .collect::<Option<Vec<_>>>()?;
    let listed: Vec<&str> = values
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();
    let lists_framing = listed.iter().any(|name| {
        FRAMING_HEADERS
            .iter()
            .any(|framing| framing.eq_ignore_ascii_case(name))
    });
    (!lists_framing).then_some(listed)
}

fn rewrite_head(
    method: &str,
    origin: &str,
    version: u8,
    target: &Target,
    headers: &[httparse::Header<'_>],
    connection_listed: &[&str],
) -> Vec<u8> {
    let host = target.host_header();
    let mut head = format!("{method} {origin} HTTP/1.{version}\r\nHost: {host}\r\n").into_bytes();
    for header in headers {
        let dropped = DROPPED_HEADERS
            .iter()
            .chain(connection_listed)
            .any(|name| name.eq_ignore_ascii_case(header.name));
        if !dropped {
            head.extend_from_slice(header.name.as_bytes());
            head.extend_from_slice(b": ");
            head.extend_from_slice(header.value);
            head.extend_from_slice(b"\r\n");
        }
    }
    head.extend_from_slice(b"Connection: close\r\n\r\n");
    head
}
