use std::path::Path;

use anyhow::{Context as _, Result};
use rustix::mount::{MountFlags, UnmountFlags};

const PROBE_DIR: &str = "/tmp/imaged-probe";

pub fn has_os_release(device: &str, fstype: &str) -> Result<bool> {
    std::fs::create_dir_all(PROBE_DIR).with_context(|| format!("creating {PROBE_DIR}"))?;
    rustix::mount::mount(device, PROBE_DIR, fstype, MountFlags::RDONLY, None)
        .with_context(|| format!("mounting {device} read-only"))?;
    let found = Path::new(PROBE_DIR)
        .join("etc/os-release")
        .symlink_metadata()
        .is_ok();
    rustix::mount::unmount(PROBE_DIR, UnmountFlags::empty())
        .with_context(|| format!("unmounting {device}"))?;
    Ok(found)
}
