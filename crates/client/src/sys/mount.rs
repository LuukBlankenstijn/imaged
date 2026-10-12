use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rustix::mount::{MountFlags, UnmountFlags};

use crate::sys::disk::{BlockDevice, PartitionTarget};
use crate::sys::fstab::plan_mounts;

const PROBE_DIR: &str = "/tmp/imaged-probe";
const ROOT_DIR: &str = "/tmp/imaged-root";

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

pub struct MountedSystem {
    root: PathBuf,
    mounted: Vec<PathBuf>,
}

impl MountedSystem {
    pub fn mount(root: &PartitionTarget, disk: &BlockDevice) -> Result<Self> {
        let mut system = MountedSystem {
            root: PathBuf::from(ROOT_DIR),
            mounted: Vec::new(),
        };
        system.attach(Path::new(""), &root.device, |target| {
            rustix::mount::mount(
                &root.device,
                target,
                &root.fstype,
                MountFlags::empty(),
                None,
            )
        })?;

        let fstab = match std::fs::read_to_string(system.root.join("etc/fstab")) {
            Ok(fstab) => fstab,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).context("reading the image's /etc/fstab"),
        };
        for planned in plan_mounts(&fstab, disk) {
            if planned.device == root.device {
                continue;
            }
            system.attach(&planned.target, &planned.device, |target| {
                rustix::mount::mount(
                    &planned.device,
                    target,
                    &planned.fstype,
                    MountFlags::empty(),
                    None,
                )
            })?;
        }

        system.attach(Path::new("proc"), "proc", |target| {
            rustix::mount::mount("proc", target, "proc", MountFlags::empty(), None)
        })?;
        for host in ["sys", "dev"] {
            system.attach(Path::new(host), host, |target| {
                rustix::mount::mount_bind_recursive(Path::new("/").join(host), target)
            })?;
        }
        Ok(system)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn attach(
        &mut self,
        relative: &Path,
        source: &str,
        mount: impl FnOnce(&Path) -> rustix::io::Result<()>,
    ) -> Result<()> {
        let target = self.root.join(relative);
        std::fs::create_dir_all(&target)
            .with_context(|| format!("creating {}", target.display()))?;
        mount(&target).with_context(|| format!("mounting {source} at {}", target.display()))?;
        tracing::info!(%source, target=%target.display(), "mounted");
        self.mounted.push(target);
        Ok(())
    }
}

impl Drop for MountedSystem {
    fn drop(&mut self) {
        while let Some(target) = self.mounted.pop() {
            if let Err(e) = rustix::mount::unmount(&target, UnmountFlags::empty()) {
                tracing::warn!(target=%target.display(), error=%e, "unmount failed, detaching instead");
                if let Err(e) = rustix::mount::unmount(&target, UnmountFlags::DETACH) {
                    tracing::error!(target=%target.display(), error=%e, "could not detach");
                }
            }
        }
        rustix::fs::sync();
    }
}
