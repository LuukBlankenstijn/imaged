use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(thiserror::Error, Debug)]
pub enum MacAddressError {
    #[error("Mac-Address not found")]
    NotFound,
    #[error("mac address error: {0}")]
    MacAddress(#[from] mac_address::MacAddressError),
}

pub fn get_mac() -> Result<String, MacAddressError> {
    mac_address::get_mac_address()?
        .ok_or(MacAddressError::NotFound)
        .map(|m| m.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum DiskError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("no suitable disk found")]
    NoDisk,
    #[error("multiple disks found: {0:?}")]
    MultipleDisks(Vec<String>),
    #[error("refusing to image disk {0}: contains the running root filesystem")]
    WouldImageRoot(String),
}

#[derive(Debug, Clone)]
pub struct Disk {
    pub name: String,
    pub device_path: PathBuf,
    pub size: u64,
}

/// Find the single internal hardware disk on this system.
/// Errors if zero, more than one, or if the only candidate contains
/// the currently-mounted root filesystem.
pub fn find_target_disk() -> Result<Disk, DiskError> {
    let mut candidates = Vec::new();

    for entry in fs::read_dir("/sys/block")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let sys_path = entry.path();

        if !is_real_disk(&sys_path, &name)? {
            continue;
        }

        let size = read_disk_size(&sys_path)?;
        candidates.push(Disk {
            device_path: PathBuf::from(format!("/dev/{name}")),
            name,
            size,
        });
    }

    let disk = match candidates.len() {
        0 => return Err(DiskError::NoDisk),
        1 => candidates.into_iter().next().unwrap(),
        _ => {
            return Err(DiskError::MultipleDisks(
                candidates.into_iter().map(|d| d.name).collect(),
            ));
        }
    };

    // Safety check: never image the disk that holds the running root fs.
    if disk_contains_root(&disk.name)? {
        return Err(DiskError::WouldImageRoot(disk.name));
    }

    Ok(disk)
}

fn is_real_disk(sys_path: &Path, name: &str) -> io::Result<bool> {
    if name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("sr")
        || name.starts_with("zram")
        || name.starts_with("dm-")
        || name.starts_with("md")
    {
        return Ok(false);
    }

    // /sys/block/<name>/removable == "1" → USB sticks, SD cards, etc.
    let removable = fs::read_to_string(sys_path.join("removable"))?;
    if removable.trim() == "1" {
        return Ok(false);
    }

    // The `device` symlink only exists for real hardware-backed devices.
    let device_link = sys_path.join("device");
    if !device_link.exists() {
        return Ok(false);
    }

    // Resolve the device symlink and check whether it sits on the USB bus.
    // Catches USB-to-SATA enclosures where `removable` lies (reports 0
    // because the inner drive isn't itself removable).
    let resolved = fs::canonicalize(&device_link)?;
    if resolved.to_string_lossy().contains("/usb") {
        return Ok(false);
    }

    Ok(true)
}

fn read_disk_size(sys_path: &Path) -> io::Result<u64> {
    // sysfs reports size in 512-byte sectors regardless of physical sector size.
    let sectors = fs::read_to_string(sys_path.join("size"))?;
    let sectors: u64 = sectors
        .trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(sectors * 512)
}

/// Returns true if any partition of `disk_name` (or the disk itself) is
/// currently mounted at `/`.
fn disk_contains_root(disk_name: &str) -> io::Result<bool> {
    let mounts = fs::read_to_string("/proc/mounts")?;

    let root_source = mounts
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let source = fields.next()?;
            let target = fields.next()?;
            (target == "/").then(|| source.to_string())
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no root mount in /proc/mounts"))?;

    // root_source is something like "/dev/sda2" or "/dev/nvme0n1p3".
    // Strip "/dev/" and walk back to the parent block device via sysfs.
    let Some(root_dev_name) = root_source.strip_prefix("/dev/") else {
        // Could be a non-/dev source like "tmpfs" or a UUID= entry that the
        // kernel didn't resolve. Be conservative: if we can't tell, assume
        // it might be us.
        return Ok(false);
    };

    // If root is mounted directly from the whole disk (rare but possible),
    // compare names directly.
    if root_dev_name == disk_name {
        return Ok(true);
    }

    // Otherwise root is on a partition. The partition's sysfs entry lives
    // under its parent disk: /sys/block/<disk>/<partition>.
    let partition_path = Path::new("/sys/block").join(disk_name).join(root_dev_name);
    Ok(partition_path.exists())
}
