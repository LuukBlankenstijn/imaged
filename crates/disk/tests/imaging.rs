use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use async_compression::tokio::bufread::ZstdDecoder;
use imaged_disk::{Filesystem, Progress, clone_compressed};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("imaged-disk-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn a_failed_partclone_fails_the_stream_instead_of_ending_it() {
    let dir = scratch("imaging");
    let source = dir.join("not-ext4.img");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(4 * 1024 * 1024)
        .unwrap();

    let mut compressed = clone_compressed(
        Filesystem::Ext4,
        &source,
        &dir.join("partclone.log"),
        |_| {},
    )
    .unwrap();
    let error = compressed.read_to_end(&mut Vec::new()).await.unwrap_err();

    assert!(
        error.to_string().contains("partclone exited with"),
        "{error}"
    );
    std::fs::remove_dir_all(Path::new(&dir)).unwrap();
}

#[tokio::test]
async fn a_capture_has_reported_its_completion_by_the_time_the_stream_ends() {
    let dir = scratch("progress");
    let source = dir.join("source.img");
    let formatted = Command::new("mke2fs")
        .args(["-q", "-t", "ext4"])
        .arg(&source)
        .arg("8M")
        .status()
        .unwrap();
    assert!(formatted.success());
    let reported = Arc::new(Mutex::new(Vec::<Progress>::new()));
    let reporting = reported.clone();

    let mut compressed = clone_compressed(
        Filesystem::Ext4,
        &source,
        &dir.join("partclone.log"),
        move |progress| reporting.lock().unwrap().push(progress),
    )
    .unwrap();
    compressed.read_to_end(&mut Vec::new()).await.unwrap();

    let reported = reported.lock().unwrap();
    assert_eq!(
        reported.last().map(|p| p.fraction),
        Some(1.0),
        "{reported:?}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

fn probe(image: &Path, tag: &str) -> String {
    let output = Command::new("blkid")
        .args(["-p", "-o", "value", "-s", tag])
        .arg(image)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[tokio::test]
async fn a_btrfs_filesystem_clones_and_restores_with_its_files_and_identity() {
    let dir = scratch("btrfs");
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("hello"), "kept\n").unwrap();
    let source = dir.join("source.img");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
    let formatted = Command::new("mkfs.btrfs")
        .args(["-q", "-L", "data", "--rootdir"])
        .arg(&root)
        .arg(&source)
        .status()
        .unwrap();
    assert!(formatted.success());

    let mut compressed = clone_compressed(
        Filesystem::Btrfs,
        &source,
        &dir.join("partclone.log"),
        |_| {},
    )
    .unwrap();
    let mut captured = Vec::new();
    compressed.read_to_end(&mut captured).await.unwrap();

    let restored = dir.join("restored.img");
    let mut partclone = tokio::process::Command::new("partclone.btrfs")
        .args(["--restore", "--restore_raw_file", "--logfile"])
        .arg(dir.join("restore.log"))
        .args(["--source", "-", "--output"])
        .arg(&restored)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = partclone.stdin.take().unwrap();
    tokio::io::copy(&mut ZstdDecoder::new(captured.as_slice()), &mut stdin)
        .await
        .unwrap();
    stdin.shutdown().await.unwrap();
    drop(stdin);
    assert!(partclone.wait().await.unwrap().success());

    let checked = Command::new("btrfs")
        .args(["check", "--readonly"])
        .arg(&restored)
        .output()
        .unwrap();
    assert!(checked.status.success(), "{checked:?}");
    let uuid = probe(&source, "UUID");
    assert!(!uuid.is_empty());
    assert_eq!(probe(&restored, "UUID"), uuid);
    assert_eq!(probe(&restored, "LABEL"), "data");
    let files = dir.join("files");
    std::fs::create_dir(&files).unwrap();
    let extracted = Command::new("btrfs")
        .arg("restore")
        .arg(&restored)
        .arg(&files)
        .output()
        .unwrap();
    assert!(extracted.status.success(), "{extracted:?}");
    assert_eq!(
        std::fs::read_to_string(files.join("hello")).unwrap(),
        "kept\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
