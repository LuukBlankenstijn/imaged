use std::time::Duration;

use imaged_e2e::harness::{self, seed};
use imaged_model::agent::path;

#[tokio::test]
async fn harness_round_trips_a_deploy_over_http() {
    let s = harness::server().await;

    let mac = harness::unique_mac();
    let host = seed::host(s, &mac, 8 * 1024 * 1024 * 1024).await;

    let parttable: Vec<u8> = b"e2e-fake-partition-table".to_vec();
    let plain = vec![0xABu8; 4096];
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("smoke-img"),
        &parttable,
        &[(1, "ext4", plain.as_slice())],
    )
    .await;

    let task_id = seed::deploy_task(s, host.id, image_id).await;

    let resp = s.agent_get(&path::partitions(task_id), &mac).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let layout: serde_json::Value = resp.json().await.expect("layout json");
    assert_eq!(layout["sector_size"], 512);
    let partitions = layout["partitions"].as_array().expect("partitions");
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0]["partition_number"], 1);
    assert_eq!(partitions[0]["fstype"], "ext4");
    assert_eq!(partitions[0]["size_bytes"], 4096);

    let resp = s.agent_get(&path::partition_table(task_id), &mac).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let table = resp.bytes().await.expect("parttable bytes");
    assert_eq!(table.as_ref(), parttable.as_slice());

    let resp = s.agent_get(&path::partition_data(task_id, 1), &mac).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let compressed = resp.bytes().await.expect("partition data bytes");
    let decompressed = seed::unzstd(&compressed).await;
    assert_eq!(decompressed, plain);

    let mut stream = harness::connect_agent_stream(s, &mac, 8 * 1024 * 1024 * 1024)
        .await
        .expect("connect agent stream");
    let event = stream
        .next_event(Duration::from_secs(5))
        .await
        .expect("first sse event");
    assert_eq!(event["Current"]["id"], task_id);
    assert_eq!(event["Current"]["work"]["Deploy"]["image_id"], image_id);
}
