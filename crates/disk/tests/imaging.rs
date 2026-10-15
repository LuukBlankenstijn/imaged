use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use imaged_disk::{Filesystem, Progress, clone_compressed};
use tokio::io::AsyncReadExt as _;

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
