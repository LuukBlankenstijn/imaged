mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{FakeBins, StubConfig, StubServer, agent_server, fake_bins, stub_server};

use imaged_agent::task::{ClientTaskExt, Task};
use imaged_model::agent::{ImagePartition, path};
use imaged_model::{Task as SharedTask, Work};

const MIB: u64 = 1024 * 1024;
const IMAGE_UUID: &str = "0b8c5c3e-7f6e-4d4b-9c1a-1e2f3a4b5c6d";
const IMAGE_LABEL: &str = "root fs";
const TASK_ID: i64 = 42;

async fn zstd_compress(data: &[u8]) -> Vec<u8> {
    use async_compression::tokio::write::ZstdEncoder;
    use tokio::io::AsyncWriteExt;

    let mut encoder = ZstdEncoder::new(Vec::new());
    encoder.write_all(data).await.expect("zstd write");
    encoder.shutdown().await.expect("zstd shutdown");
    encoder.into_inner()
}

fn ext4_image(dir: &Path) -> Vec<u8> {
    let root = dir.join("image-root");
    std::fs::create_dir_all(&root).expect("create image root");
    std::fs::write(root.join("hello"), "restored\n").expect("write image content");
    let image = dir.join("image.ext4");
    let status = Command::new("mke2fs")
        .args(["-q", "-t", "ext4", "-b", "1024", "-U", IMAGE_UUID, "-d"])
        .arg(&root)
        .arg(&image)
        .arg("8M")
        .status()
        .expect("run mke2fs");
    assert!(status.success(), "mke2fs failed: {status}");
    std::fs::read(&image).expect("read image")
}

fn debugfs(device: &Path, request: &str) -> String {
    let output = Command::new("debugfs")
        .args(["-R", request])
        .arg(device)
        .output()
        .expect("run debugfs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn block_count(device: &Path) -> u64 {
    debugfs(device, "stats")
        .lines()
        .find_map(|line| line.strip_prefix("Block count:"))
        .expect("a block count")
        .trim()
        .parse()
        .expect("a numeric block count")
}

struct Host {
    fakes: FakeBins,
    target: PathBuf,
}

fn host(fakes: FakeBins, esp_uuid: &str, esp_label: &str) -> Host {
    let dir = fakes.dir().to_str().expect("utf8 fake dir").to_string();
    let target = fakes.dir().join("disk2");
    std::fs::File::create(&target)
        .and_then(|file| file.set_len(20 * MIB))
        .expect("create the target partition");
    fakes.lsblk(&format!(
        r#"{{"blockdevices":[{{"name":"..{dir}/disk","size":{disk},"type":"disk","log-sec":512,
            "children":[
              {{"name":"..{dir}/disk1","size":{esp},"fstype":"vfat","uuid":"{esp_uuid}",
                "label":"{esp_label}","partuuid":"aaaa-01"}},
              {{"name":"..{dir}/disk2","size":{root},"fstype":"ext4","uuid":"old-root",
                "partuuid":"aaaa-02"}}
            ]}}]}}"#,
        disk = 64 * MIB,
        esp = MIB,
        root = 20 * MIB,
    ));
    Host { fakes, target }
}

async fn image_server(image: &[u8]) -> StubServer {
    stub_server(
        StubConfig::new()
            .with_partitions(vec![ImagePartition {
                id: 1,
                partition_number: 1,
                fstype: "ext4".to_string(),
                size_bytes: image.len() as u64,
                fs_uuid: Some(IMAGE_UUID.to_string()),
                fs_label: Some(IMAGE_LABEL.to_string()),
            }])
            .with_partition(1, zstd_compress(image).await),
    )
    .await
}

fn deploy_partition(server: &StubServer) -> Task {
    let agent = agent_server(&server.base_url, "aa:bb:cc:dd:ee:ff");
    let task = SharedTask::new(
        TASK_ID,
        Work::DeployPartition {
            image_id: 1,
            partuuid: "AAAA-02".to_string(),
        },
    );
    agent.status.task_started(&task);
    Task::new(agent, &task)
}

#[tokio::test]
async fn a_partition_deploy_streams_the_image_into_the_partition_and_grows_it() {
    let host = host(fake_bins().await, "ESP-1234", "ESP");
    let image = ext4_image(host.fakes.dir());
    let server = image_server(&image).await;

    deploy_partition(&server)
        .run()
        .await
        .expect("the partition deploy succeeds");

    assert_eq!(
        host.fakes.captured("partclone.extfs").as_deref(),
        Some(image.as_slice()),
        "partclone receives the decompressed image"
    );
    assert_eq!(
        block_count(&host.target),
        20 * MIB / 1024,
        "the filesystem fills the partition"
    );
    assert!(debugfs(&host.target, "stats").contains(IMAGE_UUID));
    assert_eq!(debugfs(&host.target, "cat /hello"), "restored\n");
    let requests: Vec<_> = server
        .requests()
        .into_iter()
        .map(|r| (r.method, r.path, r.mac))
        .collect();
    let mac = Some("aa:bb:cc:dd:ee:ff".to_string());
    assert_eq!(
        requests,
        [
            ("POST".to_string(), path::started(TASK_ID), mac.clone()),
            ("GET".to_string(), path::partitions(TASK_ID), mac.clone()),
            (
                "GET".to_string(),
                path::partition_data(TASK_ID, 1),
                mac.clone()
            ),
            ("GET".to_string(), path::scripts(TASK_ID), mac),
        ],
        "a partition deploy never asks for a partition table"
    );
}

async fn refused_without_writing(esp_uuid: &str, esp_label: &str, refusal: &str) {
    let host = host(fake_bins().await, esp_uuid, esp_label);
    let image = ext4_image(host.fakes.dir());
    let server = image_server(&image).await;

    let err = deploy_partition(&server).run().await.unwrap_err();

    assert!(err.to_string().contains(refusal), "{err:#}");
    assert_eq!(host.fakes.captured("partclone.extfs"), None);
    assert!(
        std::fs::read(&host.target)
            .expect("read the target")
            .iter()
            .all(|&byte| byte == 0),
        "the target partition is untouched"
    );
    let paths: Vec<_> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, [path::started(TASK_ID), path::partitions(TASK_ID)]);
}

#[tokio::test]
async fn a_partition_deploy_that_would_duplicate_a_filesystem_uuid_writes_nothing() {
    refused_without_writing(IMAGE_UUID, "ESP", "with UUID").await;
}

#[tokio::test]
async fn a_partition_deploy_that_would_duplicate_a_filesystem_label_writes_nothing() {
    refused_without_writing("ESP-1234", IMAGE_LABEL, "labelled `root fs`").await;
}
