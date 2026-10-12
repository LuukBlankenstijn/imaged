use std::path::{Component, Path, PathBuf};

use crate::sys::disk::BlockDevice;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMount {
    pub device: String,
    pub target: PathBuf,
    pub fstype: String,
}

pub fn plan_mounts(fstab: &str, disk: &BlockDevice) -> Vec<PlannedMount> {
    let mut mounts: Vec<PlannedMount> = fstab
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default();
            let mut fields = line
                .split_whitespace()
                .map(|field| String::from_utf8_lossy(&unescape(field.as_bytes())).into_owned());
            let (spec, mountpoint, fstype) = (fields.next()?, fields.next()?, fields.next()?);
            if fstype == "swap" {
                return None;
            }
            let target = relative_target(&mountpoint)?;
            let Some(partition) = find_partition(disk, &spec) else {
                tracing::info!(%spec, %mountpoint, "fstab entry is not on the target disk, not mounting it");
                return None;
            };
            let fstype = match (&partition.fstype, fstype.as_str()) {
                (Some(detected), _) => detected.clone(),
                (None, "auto") => return None,
                (None, _) => fstype,
            };
            Some(PlannedMount {
                device: partition.get_device(),
                target,
                fstype,
            })
        })
        .collect();
    mounts.sort_by_key(|mount| mount.target.components().count());
    mounts
}

fn relative_target(mountpoint: &str) -> Option<PathBuf> {
    let mut components = Path::new(mountpoint).components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    let mut target = PathBuf::new();
    for component in components {
        match component {
            Component::Normal(part) => target.push(part),
            _ => return None,
        }
    }
    (!target.as_os_str().is_empty()).then_some(target)
}

fn find_partition<'a>(disk: &'a BlockDevice, spec: &str) -> Option<&'a BlockDevice> {
    let (key, value) = match spec.split_once('=') {
        Some((key, value)) => (key.to_ascii_lowercase(), value.trim_matches('"')),
        None => {
            let path = spec.strip_prefix("/dev/")?;
            let (key, value) = match path.strip_prefix("disk/by-") {
                Some(link) => link.split_once('/')?,
                None => ("name", path),
            };
            (key.to_string(), value)
        }
    };
    disk.children.iter().find(|partition| match key.as_str() {
        "uuid" => same_id(&partition.uuid, value),
        "partuuid" => same_id(&partition.partuuid, value),
        "label" => partition.label.as_deref() == Some(value),
        "partlabel" => partition.partlabel.as_deref() == Some(value),
        "name" => partition.name == value,
        _ => false,
    })
}

fn same_id(id: &Option<String>, value: &str) -> bool {
    id.as_deref()
        .is_some_and(|id| id.eq_ignore_ascii_case(value))
}

pub(crate) fn unescape(field: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(field.len());
    let mut rest = field;
    while let Some((&first, tail)) = rest.split_first() {
        let escaped = match tail {
            [a @ b'0'..=b'3', b @ b'0'..=b'7', c @ b'0'..=b'7', ..] if first == b'\\' => {
                Some((a - b'0') * 64 + (b - b'0') * 8 + (c - b'0'))
            }
            _ => None,
        };
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                rest = &tail[3..];
            }
            None => {
                decoded.push(first);
                rest = tail;
            }
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk() -> BlockDevice {
        serde_json::from_str(
            r#"{
                "name": "nvme0n1", "size": 1000, "type": "disk",
                "children": [
                    {"name": "nvme0n1p1", "size": 10, "fstype": "vfat", "uuid": "ABCD-1234",
                     "label": "EFI", "partuuid": "11111111-01", "partlabel": "EFI System"},
                    {"name": "nvme0n1p2", "size": 900, "fstype": "ext4",
                     "uuid": "0b8c5c3e-7f6e-4d4b-9c1a-1e2f3a4b5c6d", "label": "root"},
                    {"name": "nvme0n1p3", "size": 90, "fstype": "ext4",
                     "uuid": "5f1e2d3c-0000-4000-8000-000000000003", "label": "my data",
                     "partuuid": "11111111-03", "partlabel": "srv"},
                    {"name": "nvme0n1p4", "size": 9, "fstype": "swap", "uuid": "aaaa"}
                ]
            }"#,
        )
        .expect("valid lsblk fixture")
    }

    fn planned(device: &str, target: &str, fstype: &str) -> PlannedMount {
        PlannedMount {
            device: device.to_string(),
            target: PathBuf::from(target),
            fstype: fstype.to_string(),
        }
    }

    #[test]
    fn mounts_below_the_root_are_planned_parents_first_and_the_rest_is_left_out() {
        let fstab = "\
# /etc/fstab: static file system information.
UUID=0b8c5c3e-7f6e-4d4b-9c1a-1e2f3a4b5c6d /               ext4    errors=remount-ro 0 1
UUID=ABCD-1234                            /boot/efi/linux vfat    umask=0077        0 1
LABEL=root                                /boot           auto    defaults          0 2
PARTLABEL=srv                             /srv            ext3    defaults          0 2
UUID=aaaa                                 none            swap    sw                0 0
tmpfs                                     /tmp            tmpfs   defaults          0 0
UUID=ffffffff-0000-0000-0000-000000000000 /mnt/usb        ext4    nofail            0 2
server:/export                            /mnt/nfs        nfs     defaults          0 0
";

        assert_eq!(
            plan_mounts(fstab, &disk()),
            vec![
                planned("/dev/nvme0n1p2", "boot", "ext4"),
                planned("/dev/nvme0n1p3", "srv", "ext4"),
                planned("/dev/nvme0n1p1", "boot/efi/linux", "vfat"),
            ]
        );
    }

    #[test]
    fn the_fstab_type_is_only_used_when_the_partition_has_no_detected_one() {
        let mut disk = disk();
        disk.children[2].fstype = None;

        assert_eq!(
            plan_mounts("PARTLABEL=srv /srv xfs defaults", &disk),
            vec![planned("/dev/nvme0n1p3", "srv", "xfs")]
        );
        assert!(plan_mounts("PARTLABEL=srv /srv auto defaults", &disk).is_empty());
    }

    #[test]
    fn every_way_fstab_names_a_device_resolves_to_the_partition() {
        for spec in [
            "UUID=5f1e2d3c-0000-4000-8000-000000000003",
            "UUID=\"5F1E2D3C-0000-4000-8000-000000000003\"",
            "LABEL=my\\040data",
            "PARTUUID=11111111-03",
            "PARTLABEL=srv",
            "/dev/nvme0n1p3",
            "/dev/disk/by-uuid/5f1e2d3c-0000-4000-8000-000000000003",
            "/dev/disk/by-partuuid/11111111-03",
            "/dev/disk/by-partlabel/srv",
        ] {
            assert_eq!(
                plan_mounts(&format!("{spec} /srv ext4 defaults 0 2"), &disk()),
                vec![planned("/dev/nvme0n1p3", "srv", "ext4")],
                "{spec}"
            );
        }
    }

    #[test]
    fn escaped_mount_points_are_decoded() {
        assert_eq!(
            plan_mounts("PARTLABEL=srv /srv/my\\040files ext4 defaults", &disk()),
            vec![planned("/dev/nvme0n1p3", "srv/my files", "ext4")]
        );
    }

    #[test]
    fn mount_points_that_could_leave_the_root_are_refused() {
        for mountpoint in ["/srv/../../etc", "srv", "/srv/..", "/"] {
            assert!(
                plan_mounts(&format!("PARTLABEL=srv {mountpoint} ext4"), &disk()).is_empty(),
                "{mountpoint}"
            );
        }
    }

    #[test]
    fn truncated_and_commented_lines_are_ignored() {
        assert!(
            plan_mounts("PARTLABEL=srv /srv\n#PARTLABEL=srv /srv ext4\n\n", &disk()).is_empty()
        );
        assert_eq!(
            plan_mounts("PARTLABEL=srv /srv ext4 # data", &disk()),
            vec![planned("/dev/nvme0n1p3", "srv", "ext4")]
        );
    }
}
