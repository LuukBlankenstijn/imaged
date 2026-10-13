use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, SecondsFormat, Utc};
use futures::StreamExt;
use imaged_archive::manifest::Manifest;
use imaged_archive::{
    ArchiveReader, Contents, PartitionSource, Script, archive_file_name, write_archive,
};
use imaged_model::error::{AppError, Result};
use imaged_model::script::{InputKind, ScriptInput};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio_tar::{Archive, Builder, EntryType, Header};

static DIR_ID: AtomicU64 = AtomicU64::new(0);

const TABLE: &[u8] = b"partition table";
const SMALL: &[u8] = b"second-partition-contents";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = DIR_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("imaged-archive-format-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn large() -> Vec<u8> {
    (0..600_000u32).map(|i| (i % 251) as u8).collect()
}

fn sha256(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn manifest(partitions: &[(i64, &[u8])]) -> Vec<u8> {
    manifest_with_table(TABLE, partitions)
}

fn manifest_with_table(table: &[u8], partitions: &[(i64, &[u8])]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "format": 1,
        "name": "imported",
        "captured_at": null,
        "partition_table": { "file": "parttable.bin", "sha256": sha256(table) },
        "partitions": partitions
            .iter()
            .map(|(number, data)| json!({
                "number": number,
                "fstype": "ext4",
                "size_bytes": 4096,
                "file": format!("p-{number}.pcl"),
                "sha256": sha256(data),
            }))
            .collect::<Vec<_>>(),
    }))
    .unwrap()
}

fn with_script(manifest: &[u8], name: &str, file: &str, body: &[u8]) -> Vec<u8> {
    let mut manifest: serde_json::Value = serde_json::from_slice(manifest).unwrap();
    manifest["scripts"] =
        json!([{ "name": name, "file": file, "sha256": sha256(body), "inputs": [] }]);
    serde_json::to_vec(&manifest).unwrap()
}

async fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let regular: Vec<(&str, EntryType, &[u8])> = entries
        .iter()
        .map(|&(name, data)| (name, EntryType::Regular, data))
        .collect();
    typed_tar(&regular).await
}

async fn typed_tar(entries: &[(&str, EntryType, &[u8])]) -> Vec<u8> {
    let mut builder = Builder::new_non_terminated(Vec::new());
    for &(name, entry_type, data) in entries {
        let mut header = Header::new_gnu();
        header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        header.set_entry_type(entry_type);
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append(&header, data).await.unwrap();
    }
    builder.into_inner().await.unwrap()
}

async fn untar(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = Archive::new(archive);
    let mut entries = archive.entries().unwrap();
    let mut files = Vec::new();
    while let Some(entry) = entries.next().await {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_string_lossy().into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).await.unwrap();
        files.push((name, data));
    }
    files
}

async fn open(archive: &[u8]) -> Result<ArchiveReader<'_, &[u8]>> {
    ArchiveReader::open(archive).await
}

fn invalid_argument<T>(result: Result<T>) -> String {
    match result {
        Err(AppError::InvalidArgument(message)) => message,
        Err(other) => panic!("expected an invalid argument error, got {other:?}"),
        Ok(_) => panic!("expected an invalid argument error, got success"),
    }
}

async fn rejected_on_open(archive: Vec<u8>) -> String {
    invalid_argument(open(&archive).await)
}

async fn rejected_on_unpack(archive: Vec<u8>, dir: &Path) -> String {
    let reader = open(&archive).await.unwrap();
    invalid_argument(reader.unpack(dir).await)
}

fn hostname_input() -> ScriptInput {
    ScriptInput {
        name: "hostname".into(),
        label: "Hostname".into(),
        description: Some("Name the machine announces on the network".into()),
        kind: InputKind::Text,
        required: true,
        default_value: None,
        per_machine: true,
        choices: Vec::new(),
    }
}

fn scripts() -> Vec<Script> {
    vec![
        Script {
            name: "set-hostname".into(),
            body: "#!/bin/sh\nhostname \"$IMAGED_INPUT_HOSTNAME\"\n".into(),
            inputs: vec![hostname_input()],
        },
        Script {
            name: "cleanup".into(),
            body: "rm -rf /var/cache/*\n".into(),
            inputs: vec![],
        },
    ]
}

fn captured_at() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn contents(sources: &Path) -> Contents {
    let table = sources.join("table");
    let first = sources.join("first");
    let second = sources.join("second");
    std::fs::write(&table, TABLE).unwrap();
    std::fs::write(&first, large()).unwrap();
    std::fs::write(&second, SMALL).unwrap();
    Contents {
        name: "golden".into(),
        captured_at: Some(captured_at()),
        partition_table: table,
        partitions: vec![
            PartitionSource {
                number: 1,
                fstype: "vfat".into(),
                size_bytes: 1 << 33,
                path: first,
            },
            PartitionSource {
                number: 2,
                fstype: "ntfs".into(),
                size_bytes: 1 << 33,
                path: second,
            },
        ],
        root_partition: Some(2),
        scripts: scripts(),
    }
}

async fn written(sources: &Path) -> (Manifest, Vec<u8>) {
    let mut archive = Vec::new();
    let manifest = write_archive(contents(sources), &mut archive)
        .await
        .unwrap();
    (manifest, archive)
}

#[tokio::test]
async fn written_archives_hold_the_manifest_first_followed_by_the_checksummed_files() {
    let sources = TestDir::new();
    let (manifest, archive) = written(&sources.0).await;

    let files = untar(&archive).await;

    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "manifest.json",
            "parttable.bin",
            "p-1.pcl",
            "p-2.pcl",
            "scripts/set-hostname.sh",
            "scripts/cleanup.sh"
        ]
    );
    let json: serde_json::Value = serde_json::from_slice(&files[0].1).unwrap();
    assert_eq!(
        json,
        json!({
            "format": 1,
            "name": "golden",
            "captured_at": captured_at().to_rfc3339_opts(SecondsFormat::AutoSi, true),
            "partition_table": { "file": "parttable.bin", "sha256": sha256(TABLE) },
            "partitions": [
                { "number": 1, "fstype": "vfat", "size_bytes": 1u64 << 33, "file": "p-1.pcl", "sha256": sha256(&large()) },
                { "number": 2, "fstype": "ntfs", "size_bytes": 1u64 << 33, "file": "p-2.pcl", "sha256": sha256(SMALL) },
            ],
            "root_partition": 2,
            "scripts": [
                {
                    "name": "set-hostname",
                    "file": "scripts/set-hostname.sh",
                    "sha256": sha256(&files[4].1),
                    "inputs": [{
                        "name": "hostname", "label": "Hostname",
                        "description": "Name the machine announces on the network",
                        "kind": "text", "required": true,
                        "default_value": null, "per_machine": true, "choices": []
                    }],
                },
                {
                    "name": "cleanup",
                    "file": "scripts/cleanup.sh",
                    "sha256": sha256(&files[5].1),
                    "inputs": [],
                },
            ],
        })
    );
    assert_eq!(Manifest::parse(&files[0].1).unwrap(), manifest);
    assert_eq!(files[1].1, TABLE);
    assert_eq!(files[2].1, large());
    assert_eq!(files[3].1, SMALL);
    assert_eq!(files[4].1, scripts()[0].body.as_bytes());
    assert_eq!(files[5].1, scripts()[1].body.as_bytes());
}

#[tokio::test]
async fn unpacking_a_written_archive_reproduces_its_files_and_scripts() {
    let sources = TestDir::new();
    let (manifest, archive) = written(&sources.0).await;
    let target = TestDir::new();
    let dir = target.0.join("unpacked");

    let mut input = archive.as_slice();
    let reader = ArchiveReader::open(&mut input).await.unwrap();
    assert_eq!(reader.manifest(), &manifest);
    let unpacked = reader.unpack(&dir).await.unwrap();
    assert!(input.is_empty());

    assert_eq!(unpacked.manifest, manifest);
    assert_eq!(unpacked.scripts, scripts());
    let mut stored: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|file| file.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    stored.sort();
    assert_eq!(stored, ["p-1.pcl", "p-2.pcl", "parttable.bin"]);
    assert_eq!(std::fs::read(dir.join("parttable.bin")).unwrap(), TABLE);
    assert_eq!(std::fs::read(dir.join("p-1.pcl")).unwrap(), large());
    assert_eq!(std::fs::read(dir.join("p-2.pcl")).unwrap(), SMALL);
}

#[tokio::test]
async fn verifying_checks_every_file_without_storing_any() {
    let sources = TestDir::new();
    let (expected, archive) = written(&sources.0).await;
    let mut input = archive.as_slice();
    let verified = ArchiveReader::open(&mut input)
        .await
        .unwrap()
        .verify()
        .await
        .unwrap();
    assert_eq!(verified, expected);
    assert!(input.is_empty());

    let tampered = tar(&[
        ("manifest.json", &manifest(&[(1, SMALL)])),
        ("parttable.bin", TABLE),
        ("p-1.pcl", b"tampered"),
    ])
    .await;
    assert_eq!(
        invalid_argument(open(&tampered).await.unwrap().verify().await),
        "checksum mismatch for `p-1.pcl`"
    );
}

#[tokio::test]
async fn writing_refuses_an_invalid_manifest_before_writing_any_byte() {
    let sources = TestDir::new();
    let mut bad_script = contents(&sources.0);
    bad_script.scripts[1].name = "../setup".into();
    let mut bad_root = contents(&sources.0);
    bad_root.root_partition = Some(3);

    for (contents, expected) in [
        (bad_script, "script name `../setup`"),
        (
            bad_root,
            "root partition 3 is not one of the archived partitions",
        ),
    ] {
        let mut archive = Vec::new();

        let message = invalid_argument(write_archive(contents, &mut archive).await);

        assert!(message.starts_with(expected), "{message}");
        assert!(archive.is_empty());
    }
}

#[tokio::test]
async fn script_entries_must_follow_the_naming_rules_before_unpacking() {
    for (name, file, expected) in [
        (
            "setup",
            "setup.sh",
            "script `setup` must be stored as scripts/setup.sh, not `setup.sh`",
        ),
        (
            "setup",
            "scripts/other.sh",
            "script `setup` must be stored as scripts/setup.sh, not `scripts/other.sh`",
        ),
        ("../setup", "scripts/../setup.sh", "script name `../setup`"),
    ] {
        let manifest = with_script(&manifest(&[]), name, file, b"x");
        let archive = tar(&[("manifest.json", &manifest), ("parttable.bin", TABLE)]).await;

        let message = rejected_on_open(archive).await;

        assert!(message.starts_with(expected), "{message}");
    }
}

#[tokio::test]
async fn a_root_partition_that_is_not_archived_is_rejected_before_unpacking() {
    let mut manifest: serde_json::Value = serde_json::from_slice(&manifest(&[(1, SMALL)])).unwrap();
    manifest["root_partition"] = json!(2);
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let archive = tar(&[
        ("manifest.json", &manifest),
        ("parttable.bin", TABLE),
        ("p-1.pcl", SMALL),
    ])
    .await;

    let message = rejected_on_open(archive).await;

    assert!(message.contains("root partition 2"), "{message}");
}

#[tokio::test]
async fn an_unsupported_format_is_rejected_before_unpacking() {
    let archive = tar(&[("manifest.json", br#"{"format":2,"anything":"else"}"#)]).await;

    let message = rejected_on_open(archive).await;

    assert_eq!(message, "unsupported archive format 2, expected 1");
}

#[tokio::test]
async fn an_archive_that_does_not_start_with_the_manifest_is_rejected_before_unpacking() {
    let archive = tar(&[("parttable.bin", TABLE), ("manifest.json", &manifest(&[]))]).await;

    let message = rejected_on_open(archive).await;

    assert_eq!(
        message,
        "archive must start with manifest.json, found `parttable.bin`"
    );
}

#[tokio::test]
async fn an_entry_not_listed_in_the_manifest_is_rejected_while_unpacking() {
    let target = TestDir::new();
    let archive = tar(&[
        ("manifest.json", &manifest(&[(1, SMALL)])),
        ("parttable.bin", TABLE),
        ("p-1.pcl", SMALL),
        ("notes.txt", b"hello"),
    ])
    .await;

    let message = rejected_on_unpack(archive, &target.0).await;

    assert_eq!(message, "unexpected archive entry `notes.txt`");
}

#[tokio::test]
async fn a_duplicate_entry_is_rejected_while_unpacking() {
    let target = TestDir::new();
    let archive = tar(&[
        ("manifest.json", &manifest(&[(1, SMALL)])),
        ("parttable.bin", TABLE),
        ("p-1.pcl", SMALL),
        ("p-1.pcl", SMALL),
    ])
    .await;

    let message = rejected_on_unpack(archive, &target.0).await;

    assert_eq!(message, "duplicate archive entry `p-1.pcl`");
}

#[tokio::test]
async fn a_file_listed_in_the_manifest_but_absent_is_rejected_while_unpacking() {
    let target = TestDir::new();
    let archive = tar(&[
        ("manifest.json", &manifest(&[(1, SMALL)])),
        ("parttable.bin", TABLE),
    ])
    .await;

    let message = rejected_on_unpack(archive, &target.0).await;

    assert_eq!(message, "archive is missing `p-1.pcl`");
}

#[tokio::test]
async fn entries_that_escape_the_unpack_directory_are_rejected() {
    let target = TestDir::new();
    let dir = target.0.join("unpacked");

    for path in [
        "../p-1.pcl",
        "/p-1.pcl",
        "nested/p-1.pcl",
        "scripts/../p-1.pcl",
        "scripts/nested/p-1.pcl",
    ] {
        let archive = tar(&[
            ("manifest.json", &manifest(&[(1, SMALL)])),
            ("parttable.bin", TABLE),
            (path, SMALL),
        ])
        .await;

        let message = rejected_on_unpack(archive, &dir).await;

        assert_eq!(
            message,
            format!("archive entry `{path}` is not a plain file")
        );
    }
    assert!(!target.0.join("p-1.pcl").exists());
}

#[tokio::test]
async fn a_gnu_long_name_header_is_rejected_instead_of_renaming_the_next_entry() {
    let archive = typed_tar(&[
        ("././@LongLink", EntryType::GNULongName, b"manifest.json\0"),
        ("renamed", EntryType::Regular, &manifest(&[])),
        ("parttable.bin", EntryType::Regular, TABLE),
    ])
    .await;

    let message = rejected_on_open(archive).await;

    assert_eq!(message, "archive entry `././@LongLink` is not a plain file");
}

#[tokio::test]
async fn links_extension_headers_and_directories_are_rejected_as_partition_data() {
    let target = TestDir::new();

    for entry_type in [
        EntryType::Symlink,
        EntryType::Link,
        EntryType::Directory,
        EntryType::XHeader,
        EntryType::GNUSparse,
    ] {
        let archive = typed_tar(&[
            (
                "manifest.json",
                EntryType::Regular,
                &manifest(&[(1, SMALL)]),
            ),
            ("parttable.bin", EntryType::Regular, TABLE),
            ("p-1.pcl", entry_type, SMALL),
        ])
        .await;

        let message = rejected_on_unpack(archive, &target.0).await;

        assert_eq!(
            message, "archive entry `p-1.pcl` is not a plain file",
            "{entry_type:?}"
        );
    }
}

#[tokio::test]
async fn a_partition_table_larger_than_one_mebibyte_is_rejected() {
    let target = TestDir::new();
    let accepted_dir = target.0.join("accepted");
    let largest = vec![7u8; 1024 * 1024];
    let too_large = vec![7u8; 1024 * 1024 + 1];

    open(
        &tar(&[
            ("manifest.json", &manifest_with_table(&largest, &[])),
            ("parttable.bin", &largest),
        ])
        .await,
    )
    .await
    .unwrap()
    .unpack(&accepted_dir)
    .await
    .unwrap();
    let message = rejected_on_unpack(
        tar(&[
            ("manifest.json", &manifest_with_table(&too_large, &[])),
            ("parttable.bin", &too_large),
        ])
        .await,
        &target.0.join("rejected"),
    )
    .await;

    assert_eq!(
        std::fs::read(accepted_dir.join("parttable.bin")).unwrap(),
        largest
    );
    assert_eq!(message, "parttable.bin exceeds 1048576 bytes");
}

#[test]
fn archive_file_names_only_contain_header_safe_characters() {
    assert_eq!(
        archive_file_name("Win 11 \"golden\"\r\n.v2"),
        "Win_11__golden___.v2.imaged"
    );
    assert_eq!(archive_file_name(""), "image.imaged");
}
