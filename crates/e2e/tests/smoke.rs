use std::time::Duration;

use imaged_e2e::harness::{self, seed};

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

    let resp = s
        .agent_get(&format!("/api/client/tasks/{task_id}/partitions"), &mac)
        .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let partitions: Vec<serde_json::Value> = resp.json().await.expect("partitions json");
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0]["partition_number"], 1);
    assert_eq!(partitions[0]["fstype"], "ext4");
    assert_eq!(partitions[0]["size_bytes"], 4096);

    let resp = s
        .agent_get(&format!("/api/client/tasks/{task_id}/parttable"), &mac)
        .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let table: Vec<u8> = resp.json().await.expect("parttable json");
    assert_eq!(table, parttable);

    let resp = s
        .agent_get(
            &format!("/api/client/tasks/{task_id}/partitions/1/data"),
            &mac,
        )
        .await;
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
    assert_eq!(event["Task"]["id"], task_id);
    assert_eq!(event["Task"]["task_type"], "Deploy");
    assert_eq!(event["Task"]["image_id"], image_id);
}
