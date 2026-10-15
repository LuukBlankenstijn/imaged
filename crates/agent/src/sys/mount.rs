use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rustix::fs::{Mode, OFlags, ResolveFlags};
use rustix::mount::{MountFlags, UnmountFlags};

use crate::sys::disk::{BlockDevice, PartitionTarget};
use crate::sys::fstab::{plan_mounts, unescape};

const ROOT_DIR: &str = "/tmp/imaged-root";
const STOP_ATTEMPTS: usize = 50;
const STOP_POLL: std::time::Duration = std::time::Duration::from_millis(100);

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
                root.filesystem.as_str(),
                MountFlags::empty(),
                None,
            )
        })?;

        let fstab = match read_in_root(&system.root, "etc/fstab") {
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
        if self.mounted.is_empty() {
            return Ok(());
        }
        stop_processes_inside(&self.root);
        let tracked = std::mem::take(&mut self.mounted);
        let targets = match std::fs::read("/proc/self/mountinfo") {
            Ok(mountinfo) => mounts_under(&mountinfo, &self.root),
            Err(e) => {
                tracing::error!(error=%e, "could not read /proc/self/mountinfo, unmounting only the tracked mounts");
                tracked.into_iter().rev().collect()
            }
        };
        let mut failure = None;
        for target in targets {
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

fn read_in_root(root: &Path, path: &str) -> io::Result<String> {
    let root = rustix::fs::open(
        root,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let file = rustix::fs::openat2(
        &root,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::IN_ROOT,
    )?;
    io::read_to_string(std::fs::File::from(file))
}

fn mounts_under(mountinfo: &[u8], root: &Path) -> Vec<PathBuf> {
    mountinfo
        .split(|&byte| byte == b'\n')
        .rev()
        .filter_map(|line| line.split(|&byte| byte == b' ').nth(4))
        .map(|field| PathBuf::from(OsString::from_vec(unescape(field))))
        .filter(|target| target.starts_with(root))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt as _;

    #[test]
    fn everything_mounted_below_the_root_is_unmounted_newest_first() {
        let mountinfo = b"\
22 1 0:20 / / rw - rootfs rootfs rw
40 22 254:2 / /tmp/imaged-root rw - ext4 /dev/vda2 rw
41 40 254:1 / /tmp/imaged-root/boot/efi rw - vfat /dev/vda1 rw
42 40 0:5 / /tmp/imaged-root/sys rw - sysfs none rw
43 42 0:30 / /tmp/imaged-root/sys/firmware/efi/efivars rw - efivarfs efivarfs rw
44 40 0:31 / /tmp/imaged-root/mnt/my\\040data rw - tmpfs tmpfs rw
45 40 0:32 / /tmp/imaged-root/mnt rw - tmpfs tmpfs rw
46 40 0:33 / /tmp/imaged-root/caf\xe9 rw - tmpfs tmpfs rw
47 22 0:34 / /tmp/imaged-rootless rw - tmpfs tmpfs rw
";

        let order = mounts_under(mountinfo, Path::new("/tmp/imaged-root"));

        assert_eq!(
            order,
            [
                Path::new(std::ffi::OsStr::from_bytes(b"/tmp/imaged-root/caf\xe9")),
                Path::new("/tmp/imaged-root/mnt"),
                Path::new("/tmp/imaged-root/mnt/my data"),
                Path::new("/tmp/imaged-root/sys/firmware/efi/efivars"),
                Path::new("/tmp/imaged-root/sys"),
                Path::new("/tmp/imaged-root/boot/efi"),
                Path::new("/tmp/imaged-root"),
            ]
        );
    }

    #[test]
    fn an_absolute_fstab_symlink_resolves_inside_the_image_root() {
        let root = std::env::temp_dir().join(format!("imaged-read-in-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("etc/static")).unwrap();
        std::fs::write(
            root.join("etc/static/fstab"),
            "LABEL=ESP /boot vfat defaults\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("/etc/static/fstab", root.join("etc/fstab")).unwrap();

        let fstab = read_in_root(&root, "etc/fstab");
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(fstab.unwrap(), "LABEL=ESP /boot vfat defaults\n");
    }
}
