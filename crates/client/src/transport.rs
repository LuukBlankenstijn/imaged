use std::net::IpAddr;
use std::sync::OnceLock;
pub mod multicast;

use dioxus_fullstack::{HeaderMap, HeaderValue, reqwest::Url, set_request_headers, set_server_url};
use mac_address::MacAddress;

pub fn setup_transport(base_url: Url, mac: MacAddress, ip: Option<IpAddr>) -> anyhow::Result<()> {
    let mut headers = HeaderMap::new();
    headers.insert("X-Agent-Mac", HeaderValue::from_str(&mac.to_string())?);
    if let Some(ip) = ip {
        headers.insert("X-Agent-Ip", HeaderValue::from_str(&ip.to_string())?);
    }
    set_request_headers(headers);
    static SERVER_URL: OnceLock<String> = OnceLock::new();
    set_server_url(SERVER_URL.get_or_init(|| base_url.as_str().trim_end_matches('/').to_owned()));
    Ok(())
}
