use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use imaged_disk::{Filesystem, has_os_release, holds_system, root_partition};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "imaged-disk-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn ext4_image(&self, name: &str, os_release: OsRelease) -> PathBuf {
        let root = self.0.join(format!("{name}-root"));
        let etc = root.join("etc");
        std::fs::create_dir_all(&etc).expect("create etc");
        match os_release {
            OsRelease::Regular => {
                std::fs::write(etc.join("os-release"), "ID=test\n").expect("write os-release")
            }
            OsRelease::Symlink => {
                let lib = root.join("usr/lib");
                std::fs::create_dir_all(&lib).expect("create usr/lib");
                std::fs::write(lib.join("os-release"), "ID=test\n").expect("write os-release");
                std::os::unix::fs::symlink("../usr/lib/os-release", etc.join("os-release"))
                    .expect("link os-release");
            }
            OsRelease::Dangling => {
                std::os::unix::fs::symlink("../usr/lib/os-release", etc.join("os-release"))
                    .expect("link os-release");
            }
            OsRelease::Missing => {}
        }

        let image = self.0.join(format!("{name}.img"));
        let status = Command::new("mke2fs")
            .args(["-q", "-t", "ext4", "-d"])
            .arg(&root)
            .arg(&image)
            .arg("8M")
            .status()
            .expect("run mke2fs");
        assert!(status.success(), "mke2fs failed: {status}");
        image
    }

    fn zeroed_file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, vec![0u8; 1 << 20]).expect("write zeroed file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

enum OsRelease {
    Regular,
    Symlink,
    Dangling,
    Missing,
}

#[tokio::test]
async fn a_regular_os_release_file_is_found() {
    let scratch = Scratch::new();
    let image = scratch.ext4_image("regular", OsRelease::Regular);

    assert!(has_os_release(&image).await.unwrap());
}

#[tokio::test]
async fn an_os_release_symlink_is_found() {
    let scratch = Scratch::new();
    let image = scratch.ext4_image("symlink", OsRelease::Symlink);

    assert!(has_os_release(&image).await.unwrap());
}

#[tokio::test]
async fn a_dangling_os_release_symlink_is_still_found() {
    let scratch = Scratch::new();
    let image = scratch.ext4_image("dangling", OsRelease::Dangling);

    assert!(has_os_release(&image).await.unwrap());
}

#[tokio::test]
async fn a_filesystem_without_os_release_is_not_a_system() {
    let scratch = Scratch::new();
    let image = scratch.ext4_image("missing", OsRelease::Missing);

    assert!(!has_os_release(&image).await.unwrap());
}

#[tokio::test]
async fn a_source_without_an_ext_filesystem_is_an_error() {
    let scratch = Scratch::new();
    let zeroed = scratch.zeroed_file("zeroed.img");

    let e = has_os_release(&zeroed).await.unwrap_err();

    assert!(
        e.to_string().contains("Bad magic number"),
        "unexpected error: {e:#}"
    );
}

#[tokio::test]
async fn an_ext_partition_with_os_release_holds_a_system() {
    let scratch = Scratch::new();
    let system = scratch.ext4_image("system", OsRelease::Regular);

    assert!(holds_system(2, Filesystem::Ext4, &system).await);
}

#[tokio::test]
async fn an_ext_partition_without_os_release_holds_no_system() {
    let scratch = Scratch::new();
    let data = scratch.ext4_image("data", OsRelease::Missing);

    assert!(!holds_system(1, Filesystem::Ext4, &data).await);
}

#[tokio::test]
async fn vfat_partitions_are_never_probed() {
    let scratch = Scratch::new();
    let also_a_system = scratch.ext4_image("also-a-system", OsRelease::Regular);

    let missing = Path::new("/nonexistent/imaged-disk-efi");
    assert!(!holds_system(1, Filesystem::Vfat, missing).await);
    assert!(!holds_system(2, Filesystem::Vfat, &also_a_system).await);
}

#[tokio::test]
async fn a_probe_error_on_ext_holds_no_system() {
    let scratch = Scratch::new();
    let zeroed = scratch.zeroed_file("zeroed.img");

    assert!(!holds_system(1, Filesystem::Ext4, &zeroed).await);
}

#[test]
fn the_only_system_is_the_root() {
    assert_eq!(root_partition(&[2]), Some(2));
}

#[test]
fn two_systems_have_no_root() {
    assert_eq!(root_partition(&[1, 2]), None);
}

#[test]
fn no_systems_have_no_root() {
    assert_eq!(root_partition(&[]), None);
}
