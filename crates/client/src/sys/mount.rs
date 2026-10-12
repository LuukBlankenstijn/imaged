use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rustix::mount::{MountFlags, UnmountFlags};

use crate::sys::disk::{BlockDevice, PartitionTarget};
use crate::sys::fstab::plan_mounts;

const PROBE_DIR: &str = "/tmp/imaged-probe";
const ROOT_DIR: &str = "/tmp/imaged-root";
const STOP_ATTEMPTS: usize = 50;
const STOP_POLL: std::time::Duration = std::time::Duration::from_millis(100);

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

    pub fn unmount(mut self) -> Result<()> {
        self.release()
    }

    fn release(&mut self) -> Result<()> {
        stop_processes_inside(&self.root);
        let mut failure = None;
        while let Some(target) = self.mounted.pop() {
            if let Err(e) = rustix::mount::unmount(&target, UnmountFlags::empty()) {
                let detached = rustix::mount::unmount(&target, UnmountFlags::DETACH);
                tracing::error!(target=%target.display(), error=%e, ?detached, "could not unmount");
                failure.get_or_insert(anyhow::anyhow!(
                    "could not unmount {}: {e}",
                    target.display()
                ));
            }
        }
        rustix::fs::sync();
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for MountedSystem {
    fn drop(&mut self) {
        if let Err(e) = self.release() {
            tracing::error!(error=%format!("{e:#}"), "could not undo the mounts");
        }
    }
}

fn stop_processes_inside(root: &Path) {
    for _ in 0..STOP_ATTEMPTS {
        let inside = processes_inside(root);
        if inside.is_empty() {
            return;
        }
        for pid in inside {
            tracing::warn!(
                pid = pid.as_raw_nonzero(),
                "killing a process left in the restored system"
            );
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
        std::thread::sleep(STOP_POLL);
    }
    tracing::error!(
        remaining = processes_inside(root).len(),
        "processes in the restored system did not exit"
    );
}

fn processes_inside(root: &Path) -> Vec<rustix::process::Pid> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let pid = rustix::process::Pid::from_raw(entry.file_name().to_str()?.parse().ok()?)?;
            let process_root = std::fs::read_link(entry.path().join("root")).ok()?;
            process_root.starts_with(root).then_some(pid)
        })
        .collect()
}
