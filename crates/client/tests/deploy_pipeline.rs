mod common;

use common::{StubConfig, fake_bins, init_transport, stub_server};

use imaged_client::sys::disk::PartitionTarget;
use imaged_client::task::{ClientTaskExt, DeployTask};

fn pseudo_random(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state & 0xff) as u8);
    }
    out
}

async fn zstd_compress(data: &[u8]) -> Vec<u8> {
    use async_compression::tokio::write::ZstdEncoder;
    use tokio::io::AsyncWriteExt;

    let mut encoder = ZstdEncoder::new(Vec::new());
    encoder.write_all(data).await.expect("zstd write");
    encoder.shutdown().await.expect("zstd shutdown");
    encoder.into_inner()
}

#[tokio::test]
async fn deploy_pipeline_pipes_decompressed_bytes_into_partclone() {
    let plaintext = pseudo_random(1024 * 1024);
    let compressed = zstd_compress(&plaintext).await;
    let task_id: i64 = 42;
    let cfg = StubConfig::new().with_partition(1, compressed);
    let server = stub_server(cfg).await;

    let fakes = fake_bins();
    init_transport(&server.base_url, "aa:bb:cc:dd:ee:ff");

    let target = PartitionTarget {
        device: fakes
            .dir()
            .join("target.img")
            .to_string_lossy()
            .into_owned(),
        number: 1,
        fstype: "ext4".to_string(),
        size: plaintext.len() as u64,
    };

    DeployTask::new(task_id)
        .handle_partition(target)
        .await
        .expect("handle_partition should drive the full download pipeline");

    let captured = fakes
        .captured("partclone.extfs")
        .expect("fake partclone.extfs should have captured its stdin");

    assert_eq!(
        captured.len(),
        plaintext.len(),
        "decompressed length must match the original payload"
    );
    assert_eq!(
        captured, plaintext,
        "bytes piped into partclone must equal the original plaintext"
    );

    let requests = server.requests();
    let data_path = format!("/api/client/tasks/{task_id}/partitions/1/data");
    assert!(
        requests.iter().any(|r| r.method == "GET"
            && r.path == data_path
            && r.mac.as_deref().map(str::to_ascii_lowercase).as_deref()
                == Some("aa:bb:cc:dd:ee:ff")),
        "agent must fetch the partition data with its X-Agent-Mac header, got {requests:?}"
    );
}
