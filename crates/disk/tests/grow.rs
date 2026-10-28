use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use imaged_disk::{Filesystem, grow};

const MIB: u64 = 1024 * 1024;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "imaged-disk-grow-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("root")).expect("create scratch dir");
        std::fs::write(dir.join("root/hello"), "kept\n").expect("write content");
        Self(dir)
    }

    fn ext4_in_partition(&self, filesystem_bytes: u64, partition_bytes: u64) -> PathBuf {
        let image = self.0.join("partition.img");
        let status = Command::new("mke2fs")
            .args(["-q", "-t", "ext4", "-b", "1024", "-d"])
            .arg(self.0.join("root"))
            .arg(&image)
            .arg(format!("{}k", filesystem_bytes / 1024))
            .status()
            .expect("run mke2fs");
        assert!(status.success(), "mke2fs failed: {status}");
        std::fs::File::options()
            .write(true)
            .open(&image)
            .and_then(|file| file.set_len(partition_bytes))
            .expect("extend the partition");
        image
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn debugfs(image: &Path, request: &str) -> String {
    let output = Command::new("debugfs")
        .args(["-R", request])
        .arg(image)
        .output()
        .expect("run debugfs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn block_count(image: &Path) -> u64 {
    debugfs(image, "stats")
        .lines()
        .find_map(|line| line.strip_prefix("Block count:"))
        .expect("a block count")
        .trim()
        .parse()
        .expect("a numeric block count")
}

fn clean(image: &Path) -> bool {
    Command::new("e2fsck")
        .args(["-f", "-n"])
        .arg(image)
        .output()
        .expect("run e2fsck")
        .status
        .success()
}

#[tokio::test]
async fn an_ext_filesystem_is_grown_to_fill_a_larger_partition_and_keeps_its_files() {
    let scratch = Scratch::new();
    let partition = scratch.ext4_in_partition(8 * MIB, 20 * MIB);

    grow(Filesystem::Ext4, &partition).await.expect("grow");

    assert_eq!(block_count(&partition), 20 * MIB / 1024);
    assert!(clean(&partition));
    assert_eq!(debugfs(&partition, "cat /hello"), "kept\n");
}

#[tokio::test]
async fn an_ext_filesystem_that_fills_its_partition_keeps_its_size() {
    let scratch = Scratch::new();
    let partition = scratch.ext4_in_partition(8 * MIB, 8 * MIB + 512);

    grow(Filesystem::Ext4, &partition).await.expect("grow");

    assert_eq!(block_count(&partition), 8 * MIB / 1024);
    assert!(clean(&partition));
}

#[tokio::test]
async fn a_partition_without_an_ext_filesystem_cannot_be_grown_as_ext() {
    let scratch = Scratch::new();
    let zeroed = scratch.0.join("zeroed.img");
    std::fs::write(&zeroed, vec![0u8; MIB as usize]).expect("write zeroed file");

    let e = grow(Filesystem::Ext4, &zeroed).await.unwrap_err();

    assert!(e.to_string().contains("block count"), "{e:#}");
}

#[tokio::test]
async fn vfat_and_swap_are_left_alone() {
    let missing = Path::new("/nonexistent/imaged-disk-grow");

    grow(Filesystem::Vfat, missing).await.expect("vfat");
    grow(Filesystem::Swap, missing).await.expect("swap");
}
