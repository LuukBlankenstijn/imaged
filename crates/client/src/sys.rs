pub mod disk;

use std::net::IpAddr;

use anyhow::{Context as _, Result};
use mac_address::MacAddress;

pub fn get_mac() -> Result<MacAddress> {
    mac_address::get_mac_address()
        .context("mac address")?
        .context("no mac address found")
}

pub fn get_ip() -> Option<IpAddr> {
    local_ip_address::local_ip().ok()
}

pub fn reboot() -> Result<()> {
    tracing::warn!("rebooting...");
    rustix::system::reboot(rustix::system::RebootCommand::Restart).context("could not reboot")
}
