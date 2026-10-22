pub mod disk;
pub mod fstab;
pub mod mount;

use std::net::IpAddr;

use anyhow::{Context as _, Result};
use imaged_model::agent::MacAddress;

pub fn get_mac() -> Result<MacAddress> {
    let mac = mac_address::get_mac_address()
        .context("mac address")?
        .context("no mac address found")?;
    Ok(MacAddress::from(mac.bytes()))
}

pub fn get_ip() -> Option<IpAddr> {
    local_ip_address::local_ip().ok()
}

pub fn reboot() -> Result<()> {
    tracing::warn!("rebooting...");
    rustix::system::reboot(rustix::system::RebootCommand::Restart).context("could not reboot")
}
