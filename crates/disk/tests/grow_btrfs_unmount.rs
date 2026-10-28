use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use imaged_disk::{Filesystem, grow};

const MOUNT: &str = r#"#!/bin/sh
scratch=$(dirname "$3")
echo "$scratch" > "FAKES/map/$(basename "$4")"
echo "$@" >> "$scratch/mount.log"
if [ -f "$scratch/mount.fail" ]; then echo "mount: permission denied" >&2; exit 32; fi
"#;

const UMOUNT: &str = r#"#!/bin/sh
for dir; do :; done
scratch=$(cat "FAKES/map/$(basename "$dir")")
echo "$@" >> "$scratch/umount.log"
"#;

const BTRFS: &str = r#"#!/bin/sh
if [ "$1" = inspect-internal ]; then printf 'total_bytes\t\t1048576\nsectorsize\t\t4096\n'; exit 0; fi
scratch=$(cat "FAKES/map/$(basename "$4")")
echo "$@" >> "$scratch/btrfs.log"
if [ -f "$scratch/resize.fail" ]; then echo "ERROR: resize failed" >&2; exit 1; fi
"#;

fn fake_tools() {
    static FAKES: Once = Once::new();
    FAKES.call_once(|| {
        let fakes = Path::new(env!("CARGO_TARGET_TMPDIR")).join("imaged-disk-btrfs-fakes");
        let _ = std::fs::remove_dir_all(&fakes);
        std::fs::create_dir_all(fakes.join("map")).expect("create fake tool dir");
        for (name, script) in [("mount", MOUNT), ("umount", UMOUNT), ("btrfs", BTRFS)] {
            let path = fakes.join(name);
            std::fs::write(&path, script.replace("FAKES", fakes.to_str().unwrap()))
                .expect("write fake tool");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make fake tool executable");
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let joined =
            std::env::join_paths(std::iter::once(fakes).chain(std::env::split_paths(&path)))
                .expect("join PATH");
        unsafe { std::env::set_var("PATH", joined) };
    });
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(flags: &[&str]) -> Self {
        Self::sized(flags, 320 * 1024 * 1024)
    }

    fn sized(flags: &[&str], partition_bytes: u64) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        fake_tools();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "imaged-disk-grow-unmount-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        std::fs::File::create(dir.join("partition.img"))
            .and_then(|file| file.set_len(partition_bytes))
            .expect("create the partition");
        for flag in flags {
            std::fs::write(dir.join(flag), "").expect("write flag");
        }
        Self(dir)
    }

    fn device(&self) -> PathBuf {
        self.0.join("partition.img")
    }

    fn logged(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.0.join(name))
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    fn mount_point(&self) -> PathBuf {
        let [mounted] = self.logged("mount.log").try_into().expect("one mount");
        let device = self.device();
        let expected = format!("-t btrfs {} ", device.display());
        PathBuf::from(mounted.strip_prefix(&expected).expect("mounted the device"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn a_grown_btrfs_filesystem_is_unmounted_and_its_mount_point_removed() {
    let scratch = Scratch::new(&[]);

    grow(Filesystem::Btrfs, &scratch.device())
        .await
        .expect("grow");

    let mount_point = scratch.mount_point();
    assert_eq!(
        scratch.logged("btrfs.log"),
        [format!("filesystem resize max {}", mount_point.display())]
    );
    assert_eq!(
        scratch.logged("umount.log"),
        [mount_point.display().to_string()]
    );
    assert!(!mount_point.exists());
}

#[tokio::test]
async fn a_partition_below_the_kernel_resize_minimum_is_left_unmounted_and_ungrown() {
    let scratch = Scratch::sized(&[], 200 * 1024 * 1024);

    grow(Filesystem::Btrfs, &scratch.device())
        .await
        .expect("grow");

    assert!(scratch.logged("mount.log").is_empty());
    assert!(
        scratch
            .logged("btrfs.log")
            .iter()
            .all(|call| !call.contains("resize"))
    );
}

#[tokio::test]
async fn a_failed_resize_still_unmounts_the_filesystem_and_removes_its_mount_point() {
    let scratch = Scratch::new(&["resize.fail"]);

    let e = grow(Filesystem::Btrfs, &scratch.device())
        .await
        .unwrap_err();

    let mount_point = scratch.mount_point();
    assert!(
        e.to_string().starts_with(&format!(
            "btrfs filesystem resize max {} failed",
            mount_point.display()
        )),
        "{e:#}"
    );
    assert!(e.to_string().ends_with("ERROR: resize failed"), "{e:#}");
    assert_eq!(
        scratch.logged("umount.log"),
        [mount_point.display().to_string()]
    );
    assert!(!mount_point.exists());
}

#[tokio::test]
async fn a_failed_mount_removes_the_mount_point_without_resizing_or_unmounting() {
    let scratch = Scratch::new(&["mount.fail"]);

    let e = grow(Filesystem::Btrfs, &scratch.device())
        .await
        .unwrap_err();

    let mount_point = scratch.mount_point();
    assert!(
        e.to_string().starts_with(&format!(
            "mount -t btrfs {} {} failed",
            scratch.device().display(),
            mount_point.display()
        )),
        "{e:#}"
    );
    assert!(scratch.logged("btrfs.log").is_empty());
    assert!(scratch.logged("umount.log").is_empty());
    assert!(!mount_point.exists());
}
