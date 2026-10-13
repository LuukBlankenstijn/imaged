use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use async_compression::tokio::bufread::ZstdDecoder;
use futures::StreamExt;
use imaged_archive::manifest::Manifest;
use imaged_archive::{ArchiveReader, Unpacked};
use imaged_disk::Filesystem;
use imaged_model::script::{InputKind, ScriptInput};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;

const MIB: u64 = 1024 * 1024;

struct Disk {
    path: PathBuf,
    partitions: Vec<(u64, u64)>,
}

impl Disk {
    fn new(dir: &Path) -> Self {
        let path = dir.join("disk.img");
        File::create(&path).unwrap().set_len(64 * MIB).unwrap();
        run(Command::new("sgdisk")
            .args([
                "-n1:1MiB:+16MiB",
                "-t1:ef00",
                "-n2:0:+24MiB",
                "-t2:8300",
                "-n3:0:0",
            ])
            .arg(&path));
        let dump: serde_json::Value =
            serde_json::from_slice(&run(Command::new("sfdisk").arg("--json").arg(&path)).stdout)
                .unwrap();
        let table = &dump["partitiontable"];
        let sector = table["sectorsize"].as_u64().unwrap_or(512);
        let partitions = table["partitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["start"].as_u64().unwrap() * sector,
                    p["size"].as_u64().unwrap() * sector,
                )
            })
            .collect();
        Self { path, partitions }
    }

    fn size(&self, number: usize) -> u64 {
        self.partitions[number - 1].1
    }

    fn write(&self, number: usize, image: &Path) {
        let (offset, size) = self.partitions[number - 1];
        let mut source = File::open(image).unwrap();
        assert!(source.metadata().unwrap().len() <= size);
        let mut disk = OpenOptions::new().write(true).open(&self.path).unwrap();
        disk.seek(SeekFrom::Start(offset)).unwrap();
        std::io::copy(&mut source, &mut disk).unwrap();
    }
}

fn run(command: &mut Command) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("could not run {command:?}: {e}"));
    assert!(
        output.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn vfat_image(dir: &Path, size: u64, file: &Path) -> PathBuf {
    let image = dir.join("vfat.img");
    run(Command::new("mkfs.vfat")
        .arg("-C")
        .arg(&image)
        .arg((size / 1024).to_string()));
    run(Command::new("mcopy")
        .env("MTOOLS_SKIP_CHECK", "1")
        .arg("-i")
        .arg(&image)
        .arg(file)
        .arg("::/"));
    image
}

fn ext4_image(dir: &Path, name: &str, size: u64, root: &Path) -> PathBuf {
    let image = dir.join(format!("{name}.img"));
    run(Command::new("mke2fs")
        .args(["-q", "-t", "ext4", "-d"])
        .arg(root)
        .arg(&image)
        .arg(format!("{}k", size / 1024)));
    image
}

fn os_root(dir: &Path, name: &str, marker: &str) -> PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(root.join("etc")).unwrap();
    std::fs::create_dir_all(root.join("usr/lib")).unwrap();
    std::fs::write(root.join("usr/lib/os-release"), "ID=test\n").unwrap();
    std::os::unix::fs::symlink("../usr/lib/os-release", root.join("etc/os-release")).unwrap();
    std::fs::write(root.join("marker"), marker).unwrap();
    root
}

fn swap_image(dir: &Path, size: u64) -> PathBuf {
    let image = dir.join("swap.img");
    File::create(&image).unwrap().set_len(size).unwrap();
    run(Command::new("mkswap").arg(&image));
    image
}

fn pack<I, S>(disk: &Path, output: &Path, extra: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_imaged-pack"))
        .arg(disk)
        .args(["--name", "packed"])
        .args(extra)
        .arg("-o")
        .arg(output)
        .output()
        .unwrap()
}

fn packed(output: Output) {
    assert!(
        output.status.success(),
        "imaged-pack failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn partial(output: &Path) -> PathBuf {
    let mut partial = output.as_os_str().to_owned();
    partial.push(".partial");
    partial.into()
}

fn assert_failed_without_output(output: Output, message: &str, archive: &Path) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "imaged-pack succeeded");
    assert!(
        stderr.contains(message),
        "`{message}` missing from: {stderr}"
    );
    assert!(!archive.exists());
    assert!(!partial(archive).exists());
}

async fn manifest(archive: &Path) -> Manifest {
    let file = tokio::fs::File::open(archive).await.unwrap();
    ArchiveReader::open(file).await.unwrap().manifest().clone()
}

async fn unpack(archive: &Path, dir: &Path) -> Unpacked {
    let file = tokio::fs::File::open(archive).await.unwrap();
    ArchiveReader::open(file)
        .await
        .unwrap()
        .unpack(dir)
        .await
        .unwrap()
}

async fn entry_names(archive: &Path) -> Vec<String> {
    let file = tokio::fs::File::open(archive).await.unwrap();
    let mut archive = tokio_tar::Archive::new(file);
    let mut entries = archive.entries().unwrap();
    let mut names = Vec::new();
    while let Some(entry) = entries.next().await {
        names.push(
            entry
                .unwrap()
                .path()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
    }
    names
}

async fn restore(filesystem: Filesystem, compressed: &Path, dir: &Path) -> PathBuf {
    let target = dir.join(format!("restored-{filesystem}.img"));
    let log = dir.join("restore.log");
    assert!(!target.exists());
    let compressed = tokio::fs::File::open(compressed).await.unwrap();
    let mut decoder = ZstdDecoder::new(tokio::io::BufReader::new(compressed));
    let mut child = tokio::process::Command::new(filesystem.partclone_binary())
        .arg("--restore")
        .arg("--restore_raw_file")
        .arg("--logfile")
        .arg(&log)
        .args(["--source", "-", "--output"])
        .arg(&target)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let copied = {
        let mut stdin = child.stdin.take().unwrap();
        match tokio::io::copy(&mut decoder, &mut stdin).await {
            Ok(_) => stdin.shutdown().await,
            Err(error) => Err(error),
        }
    };
    let status = child.wait().await.unwrap();
    assert!(
        status.success(),
        "{} --restore failed: {status}\n{}",
        filesystem.partclone_binary(),
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    copied.unwrap();
    target
}

fn two_systems(dir: &Path) -> Disk {
    let disk = Disk::new(dir);
    let one = os_root(dir, "one-root", "one");
    let two = os_root(dir, "two-root", "two");
    disk.write(1, &ext4_image(dir, "one", disk.size(1), &one));
    disk.write(2, &ext4_image(dir, "two", disk.size(2), &two));
    disk
}

#[tokio::test]
async fn a_disk_image_packs_like_a_capture() {
    let dir = TempDir::new().unwrap();
    let dir = dir.path();
    let disk = Disk::new(dir);
    let greeting = dir.join("hello.txt");
    std::fs::write(&greeting, "hello from the esp\n").unwrap();
    disk.write(1, &vfat_image(dir, disk.size(1), &greeting));
    let root = os_root(dir, "root", "packed by imaged-pack\n");
    disk.write(2, &ext4_image(dir, "root", disk.size(2), &root));

    let zeta = dir.join("zeta.sh");
    std::fs::write(&zeta, "#!/bin/sh\nhostnamectl hostname \"$hostname\"\n").unwrap();
    std::fs::write(
        dir.join("zeta.inputs.json"),
        r#"[{"name": "hostname", "label": "Hostname", "description": "Name the machine announces", "kind": "text", "required": true, "per_machine": true}]"#,
    )
    .unwrap();
    let alpha = dir.join("alpha.sh");
    std::fs::write(&alpha, "#!/bin/sh\necho alpha\n").unwrap();

    let archive = dir.join("image.imaged");
    packed(pack(
        &disk.path,
        &archive,
        [
            OsStr::new("--script"),
            zeta.as_os_str(),
            OsStr::new("--script"),
            alpha.as_os_str(),
        ],
    ));
    assert!(!partial(&archive).exists());

    assert_eq!(
        entry_names(&archive).await,
        [
            "manifest.json",
            "parttable.bin",
            "p-1.pcl",
            "p-2.pcl",
            "scripts/zeta.sh",
            "scripts/alpha.sh",
        ]
    );

    let unpacked_dir = dir.join("unpacked");
    let Unpacked { manifest, scripts } = unpack(&archive, &unpacked_dir).await;
    assert_eq!(manifest.name, "packed");
    assert!(manifest.captured_at.is_some());
    let partitions: Vec<(i64, &str, u64)> = manifest
        .partitions
        .iter()
        .map(|p| (p.number, p.fstype.as_str(), p.size_bytes))
        .collect();
    assert_eq!(
        partitions,
        [(1, "vfat", disk.size(1)), (2, "ext4", disk.size(2))]
    );
    assert_eq!(disk.size(1), 16 * MIB);
    assert_eq!(manifest.root_partition, Some(2));

    let names: Vec<&str> = scripts.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["zeta", "alpha"]);
    assert_eq!(scripts[0].body, std::fs::read_to_string(&zeta).unwrap());
    assert_eq!(
        scripts[0].inputs,
        [ScriptInput {
            name: "hostname".to_string(),
            label: "Hostname".to_string(),
            description: Some("Name the machine announces".to_string()),
            kind: InputKind::Text,
            required: true,
            default_value: None,
            per_machine: true,
            choices: Vec::new(),
        }]
    );
    assert_eq!(scripts[1].body, std::fs::read_to_string(&alpha).unwrap());
    assert!(scripts[1].inputs.is_empty());

    let expected_table = dir.join("expected-parttable.bin");
    let mut backup = std::ffi::OsString::from("--backup=");
    backup.push(&expected_table);
    run(Command::new("sgdisk").arg(backup).arg(&disk.path));
    assert_eq!(
        std::fs::read(unpacked_dir.join("parttable.bin")).unwrap(),
        std::fs::read(&expected_table).unwrap()
    );

    let ext4 = restore(Filesystem::Ext4, &unpacked_dir.join("p-2.pcl"), dir).await;
    run(Command::new("e2fsck").arg("-fn").arg(&ext4));
    let marker = run(Command::new("debugfs")
        .args(["-R", "cat /marker"])
        .arg(&ext4));
    assert_eq!(
        String::from_utf8_lossy(&marker.stdout),
        "packed by imaged-pack\n"
    );

    let vfat = restore(Filesystem::Vfat, &unpacked_dir.join("p-1.pcl"), dir).await;
    run(Command::new("fsck.fat").arg("-n").arg(&vfat));
    let hello = run(Command::new("mtype")
        .env("MTOOLS_SKIP_CHECK", "1")
        .arg("-i")
        .arg(&vfat)
        .arg("::/hello.txt"));
    assert_eq!(
        String::from_utf8_lossy(&hello.stdout),
        "hello from the esp\n"
    );
}

#[tokio::test]
async fn an_unsupported_filesystem_fails_without_leaving_output() {
    let dir = TempDir::new().unwrap();
    let dir = dir.path();
    let disk = Disk::new(dir);
    disk.write(2, &swap_image(dir, disk.size(2)));

    let archive = dir.join("image.imaged");
    assert_failed_without_output(
        pack(&disk.path, &archive, [] as [&str; 0]),
        "filesystem not supported: swap",
        &archive,
    );
}

#[tokio::test]
async fn two_operating_systems_leave_the_root_partition_unset() {
    let dir = TempDir::new().unwrap();
    let dir = dir.path();
    let disk = two_systems(dir);

    let archive = dir.join("image.imaged");
    packed(pack(&disk.path, &archive, [] as [&str; 0]));
    let manifest = manifest(&archive).await;
    let numbers: Vec<i64> = manifest.partitions.iter().map(|p| p.number).collect();
    assert_eq!(numbers, [1, 2]);
    assert_eq!(manifest.root_partition, None);
}

#[tokio::test]
async fn the_root_partition_can_be_chosen_among_the_packed_partitions() {
    let dir = TempDir::new().unwrap();
    let dir = dir.path();
    let disk = two_systems(dir);

    let chosen = dir.join("chosen.imaged");
    packed(pack(&disk.path, &chosen, ["--root-partition", "2"]));
    assert_eq!(manifest(&chosen).await.root_partition, Some(2));

    let missing = dir.join("missing.imaged");
    assert_failed_without_output(
        pack(&disk.path, &missing, ["--root-partition", "9"]),
        "root partition 9",
        &missing,
    );
}

#[tokio::test]
async fn invalid_scripts_fail_before_any_output_is_written() {
    let dir = TempDir::new().unwrap();
    let dir = dir.path();
    let disk = Disk::new(dir);

    let bad_name = dir.join("bad name.sh");
    std::fs::write(&bad_name, "echo hi\n").unwrap();
    let archive = dir.join("bad-name.imaged");
    assert_failed_without_output(
        pack(
            &disk.path,
            &archive,
            [OsStr::new("--script"), bad_name.as_os_str()],
        ),
        "script name `bad name`",
        &archive,
    );

    let mode = dir.join("mode.sh");
    std::fs::write(&mode, "echo \"$mode\"\n").unwrap();
    std::fs::write(
        dir.join("mode.inputs.json"),
        r#"[{"name": "mode", "kind": "choice"}]"#,
    )
    .unwrap();
    let archive = dir.join("no-choices.imaged");
    assert_failed_without_output(
        pack(
            &disk.path,
            &archive,
            [OsStr::new("--script"), mode.as_os_str()],
        ),
        "choice input `mode` needs at least one choice",
        &archive,
    );

    let first = dir.join("first");
    let second = dir.join("second");
    for copy in [&first, &second] {
        std::fs::create_dir(copy).unwrap();
        std::fs::write(copy.join("setup.sh"), "echo setup\n").unwrap();
    }
    let archive = dir.join("duplicate.imaged");
    let first = first.join("setup.sh");
    let second = second.join("setup.sh");
    assert_failed_without_output(
        pack(
            &disk.path,
            &archive,
            [
                OsStr::new("--script"),
                first.as_os_str(),
                OsStr::new("--script"),
                second.as_os_str(),
            ],
        ),
        "script `setup` is given more than once",
        &archive,
    );
}
