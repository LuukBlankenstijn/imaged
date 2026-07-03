pub mod disk;

use anyhow::{Context as _, Result};

pub fn get_mac() -> Result<String> {
    Ok(mac_address::get_mac_address()
        .context("mac address")?
        .context("no mac address found")?
        .to_string())
}

pub fn reboot() -> Result<()> {
    tracing::warn!("rebooting...");
    rustix::system::reboot(rustix::system::RebootCommand::Restart).context("could not reboot")
}
