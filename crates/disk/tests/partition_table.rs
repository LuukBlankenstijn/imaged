use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use imaged_disk::{Partition, PartitionTable, backup_partition_table, restore_partition_table};

const MIB: u64 = 1024 * 1024;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "imaged-disk-table-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn disk(&self, name: &str, size: u64) -> PathBuf {
        let path = self.0.join(name);
        std::fs::File::create(&path)
            .and_then(|file| file.set_len(size))
            .expect("create disk image");
        path
    }

    fn gpt_disk(&self, name: &str, size: u64) -> PathBuf {
        let disk = self.disk(name, size);
        run(Command::new("sgdisk")
            .args(["-n1:1MiB:+16MiB", "-t1:ef00", "-n2:0:0", "-t2:8300"])
            .arg(&disk));
        disk
    }

    fn mbr_disk(&self, name: &str) -> PathBuf {
        let disk = self.disk(name, 8 * MIB);
        let mut sfdisk = Command::new("sfdisk")
            .arg("-q")
            .arg(&disk)
            .stdin(Stdio::piped())
            .spawn()
            .expect("run sfdisk");
        std::io::Write::write_all(
            sfdisk.stdin.as_mut().expect("sfdisk stdin"),
            b"label: dos\n,2M,83\n,,83\n",
        )
        .expect("write sfdisk script");
        assert!(sfdisk.wait().expect("wait for sfdisk").success());
        disk
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(command: &mut Command) {
    let output = command.output().expect("run command");
    assert!(
        output.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn backup(disk: &Path, scratch: &Scratch) -> (PathBuf, PartitionTable) {
    let backup = scratch.0.join(format!(
        "{}.parttable",
        disk.file_name().unwrap().to_string_lossy()
    ));
    let table = backup_partition_table(disk, &backup).await.unwrap();
    (backup, table)
}

async fn assert_refused_untouched(target: &Path, backup: &Path, sector_size: u32, message: &str) {
    let before = std::fs::read(target).unwrap();

    let error = restore_partition_table(target, backup, sector_size)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains(message), "`{message}` missing from: {error}");
    assert!(
        std::fs::read(target).unwrap() == before,
        "{target:?} was modified"
    );
}

#[tokio::test]
async fn a_gpt_backup_reports_its_partitions_in_bytes() {
    let scratch = Scratch::new();
    let disk = scratch.gpt_disk("source.img", 64 * MIB);

    let (backup, table) = backup(&disk, &scratch).await;

    assert!(backup.exists());
    assert_eq!(
        table,
        PartitionTable {
            sector_size: 512,
            partitions: vec![
                Partition {
                    number: 1,
                    offset: MIB,
                    size_bytes: 16 * MIB,
                },
                Partition {
                    number: 2,
                    offset: 17 * MIB,
                    size_bytes: 64 * MIB - 17 * MIB - 33 * 512,
                },
            ],
        }
    );
}

#[tokio::test]
async fn disks_without_a_gpt_are_refused_before_anything_is_backed_up() {
    let scratch = Scratch::new();
    for (disk, label) in [
        (scratch.mbr_disk("mbr.img"), "dos"),
        (scratch.disk("blank.img", 8 * MIB), "no partition table"),
    ] {
        let backup = scratch.0.join("parttable.bin");

        let error = backup_partition_table(&disk, &backup).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("only GPT disks are supported, found {label}")
        );
        assert!(!backup.exists());
    }
}

#[tokio::test]
async fn restoring_onto_a_larger_disk_reproduces_the_partitions() {
    let scratch = Scratch::new();
    let (backup, source) = backup(&scratch.gpt_disk("source.img", 64 * MIB), &scratch).await;
    let target = scratch.disk("target.img", 96 * MIB);

    restore_partition_table(&target, &backup, source.sector_size)
        .await
        .unwrap();

    let (_, restored) = self::backup(&target, &scratch).await;
    assert_eq!(restored, source);
}

#[tokio::test]
async fn a_target_too_small_for_the_partitions_is_refused_before_it_is_touched() {
    let scratch = Scratch::new();
    let (backup, source) = backup(&scratch.gpt_disk("source.img", 64 * MIB), &scratch).await;
    let target = scratch.gpt_disk("target.img", 32 * MIB);

    assert_refused_untouched(
        &target,
        &backup,
        source.sector_size,
        &format!(
            "holds {} bytes, the image's partitions need at least {}",
            32 * MIB,
            64 * MIB
        ),
    )
    .await;
}

#[tokio::test]
async fn a_target_with_another_sector_size_is_refused_before_it_is_touched() {
    let scratch = Scratch::new();
    let (backup, _) = backup(&scratch.gpt_disk("source.img", 64 * MIB), &scratch).await;
    let target = scratch.gpt_disk("target.img", 64 * MIB);

    assert_refused_untouched(
        &target,
        &backup,
        4096,
        "the image's partition table is for 4096-byte sectors",
    )
    .await;
}

#[tokio::test]
async fn a_damaged_backup_is_refused_before_the_target_is_touched() {
    let scratch = Scratch::new();
    let (backup, source) = backup(&scratch.gpt_disk("source.img", 64 * MIB), &scratch).await;
    let truncated = scratch.0.join("truncated.parttable");
    std::fs::write(&truncated, &std::fs::read(&backup).unwrap()[..4096]).unwrap();
    let target = scratch.gpt_disk("target.img", 64 * MIB);

    assert_refused_untouched(
        &target,
        &truncated,
        source.sector_size,
        "the partition table backup ends inside its partition entries",
    )
    .await;
}
