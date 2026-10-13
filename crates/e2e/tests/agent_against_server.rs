use std::collections::BTreeMap;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use mac_address::MacAddress;
use reqwest::Url;
use tokio::io::AsyncReadExt;

use imaged_agent::server::Server;
use imaged_core::domain::image::ImageStatus;
use imaged_core::domain::script::NewScript;
use imaged_core::domain::task::{Task, TaskState, TaskType};
use imaged_e2e::harness::{self, seed};
use imaged_model::ServerEvent;
use imaged_model::agent::{ImagePartition, PostInstall, PostInstallScript};
use imaged_model::script::{InputKind, InputValues, ScriptInput};

const DISK: u64 = 8 * 1024 * 1024 * 1024;
const AGENT_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7));
const WAIT: Duration = Duration::from_secs(5);

fn agent(s: &harness::TestServer) -> (Server, String) {
    let mac: MacAddress = harness::unique_mac().parse().expect("unique mac parses");
    let base = Url::parse(&s.base_url).expect("harness base url");
    let server = Server::new(base, mac, Some(AGENT_IP)).expect("agent client");
    (server, mac.to_string())
}

fn host_state(task: &Task, host_id: i64) -> TaskState {
    task.hosts
        .iter()
        .find(|h| h.host_id == host_id)
        .expect("host row present on task")
        .state
}

async fn stored_partitions(s: &harness::TestServer, image_id: i64) -> Vec<ImagePartition> {
    s.container
        .image_repo
        .get_partitions(image_id)
        .await
        .expect("image partitions")
        .into_iter()
        .map(Into::into)
        .collect()
}

#[tokio::test]
async fn connect_events_registers_an_unknown_host_with_its_disk_size_and_ip() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    assert!(s.container.host_repo.get_by_mac(&mac).await.is_err());

    let events = agent.connect_events(DISK).await.expect("connect events");
    let mut events = std::pin::pin!(events);

    let host = s
        .container
        .host_repo
        .get_by_mac(&mac)
        .await
        .expect("host registered by the event stream");
    assert_eq!(host.mac_address, mac);
    assert_eq!(host.disk_size, DISK);
    assert_eq!(host.ip.as_deref(), Some("10.0.0.7"));

    assert!(
        tokio::time::timeout(Duration::from_millis(300), events.next())
            .await
            .is_err(),
        "a host without tasks receives no event"
    );
}

#[tokio::test]
async fn the_pending_capture_task_is_the_first_server_event() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    let host = seed::host(s, &mac, 1024).await;
    let image = s
        .container
        .image_repo
        .create_image(harness::unique_name("rc-events"))
        .await
        .expect("create image");
    let task_id = seed::capture_task(s, host.id, image.id).await;

    let events = agent.connect_events(DISK).await.expect("connect events");
    let mut events = std::pin::pin!(events);
    let event = tokio::time::timeout(WAIT, events.next())
        .await
        .expect("first event within the timeout")
        .expect("stream still open");

    let ServerEvent::Task(task) = event else {
        panic!("expected a task event, got {event:?}");
    };
    assert_eq!(task.id, task_id);
    assert_eq!(task.task_type, imaged_model::TaskType::Capture);
    assert_eq!(task.image_id, Some(image.id));

    let host = s.container.host_repo.get_by_mac(&mac).await.unwrap();
    assert_eq!(host.disk_size, DISK, "connecting refreshes the disk size");
}

#[tokio::test]
async fn a_capture_through_the_client_yields_a_ready_image_holding_the_uploaded_bytes() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    let host = seed::host(s, &mac, DISK).await;
    let image = s
        .container
        .image_repo
        .create_image(harness::unique_name("rc-capture"))
        .await
        .expect("create image");
    let task_id = seed::capture_task(s, host.id, image.id).await;

    let table = b"real-client-partition-table".to_vec();
    agent
        .upload_partition_table(task_id, Bytes::from(table.clone()))
        .await
        .expect("upload partition table");
    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Running);

    let plain: Vec<u8> = (0..20_000u32).map(|i| (i % 241) as u8).collect();
    let compressed = seed::zstd(&plain).await;
    let uploaded = agent
        .upload_partition_data(
            task_id,
            1,
            "ext4".to_string(),
            plain.len() as i64,
            Cursor::new(compressed.clone()),
        )
        .await
        .expect("upload partition data");
    assert_eq!(uploaded.partition_number, 1);
    assert_eq!(uploaded.fstype, "ext4");
    assert_eq!(uploaded.size_bytes, plain.len() as u64);
    assert_eq!(stored_partitions(s, image.id).await, vec![uploaded.clone()]);

    agent
        .report_root_partition(task_id, 1)
        .await
        .expect("report root partition");
    agent.mark_finished(task_id).await.expect("mark finished");

    let stored = s.container.image_repo.get(image.id).await.unwrap();
    assert_eq!(stored.status, ImageStatus::Ready);
    assert!(stored.captured_at.is_some());
    assert_eq!(stored.root_partition, Some(1));
    let partitions: Vec<ImagePartition> = stored.partitions.into_iter().map(Into::into).collect();
    assert_eq!(partitions, vec![uploaded]);

    let on_disk = tokio::fs::read(s.container.image_service.get_partition_path(image.id, 1))
        .await
        .expect("read stored partition");
    assert_eq!(on_disk, compressed);
    assert_eq!(seed::unzstd(&on_disk).await, plain);
    let on_disk_table =
        tokio::fs::read(s.container.image_service.get_partition_table_path(image.id))
            .await
            .expect("read stored partition table");
    assert_eq!(on_disk_table, table);

    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Done);
}

#[tokio::test]
async fn a_restore_through_the_client_downloads_the_image_and_records_the_script_run() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    let host = seed::host(s, &mac, DISK).await;

    let table: Vec<u8> = (0..300u32).map(|i| (i % 256) as u8).collect();
    let ext4: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    let vfat: Vec<u8> = (0..4096u32).map(|i| ((i * 7 + 3) % 253) as u8).collect();
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("rc-restore"),
        &table,
        &[(1, "ext4", ext4.as_slice()), (2, "vfat", vfat.as_slice())],
    )
    .await;
    s.container
        .image_repo
        .set_root_partition(image_id, Some(1))
        .await
        .expect("set root partition");

    let script = s
        .container
        .script_repo
        .create(&NewScript {
            name: harness::unique_name("rc-setup"),
            body: "#!/bin/sh\necho \"$hostname\"\n".to_string(),
            inputs: vec![ScriptInput {
                name: "hostname".to_string(),
                label: "Hostname".to_string(),
                kind: InputKind::Text,
                required: true,
                default_value: None,
                per_machine: true,
                choices: Vec::new(),
            }],
        })
        .await
        .expect("create script");
    s.container
        .script_repo
        .set_for_image(image_id, &[script.id])
        .await
        .expect("attach script to image");

    let values = InputValues::from([("hostname".to_string(), "pc-42".to_string())]);
    let task_id = s
        .container
        .task_repo
        .create_with_inputs(
            TaskType::Deploy,
            vec![host.id],
            Some(image_id),
            &BTreeMap::from([(host.id, values)]),
        )
        .await
        .expect("create deploy task")
        .id;

    let partitions = agent.partitions(task_id).await.expect("list partitions");
    assert_eq!(partitions, stored_partitions(s, image_id).await);
    assert_eq!(
        partitions
            .iter()
            .map(|p| (p.partition_number, p.fstype.as_str(), p.size_bytes))
            .collect::<Vec<_>>(),
        vec![
            (1, "ext4", ext4.len() as u64),
            (2, "vfat", vfat.len() as u64)
        ]
    );

    let downloaded_table = agent
        .partition_table(task_id)
        .await
        .expect("download partition table");
    assert_eq!(downloaded_table.as_ref(), table.as_slice());
    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Running);

    for (number, plain) in [(1i64, &ext4), (2i64, &vfat)] {
        let mut reader = agent
            .partition_data(task_id, number)
            .await
            .expect("download partition data");
        let mut compressed = Vec::new();
        reader
            .read_to_end(&mut compressed)
            .await
            .expect("read partition data");
        assert_eq!(&seed::unzstd(&compressed).await, plain);
    }

    let post_install = agent.post_install(task_id).await.expect("post install");
    assert_eq!(
        post_install,
        PostInstall {
            root_partition: Some(1),
            proxy: s.container.script_proxy.as_deref().map(str::to_owned),
            variables: (*s.container.script_variables).clone(),
            scripts: vec![PostInstallScript {
                position: 0,
                name: script.name.clone(),
                body: script.body.clone(),
                inputs: BTreeMap::from([("hostname".to_string(), "pc-42".to_string())]),
            }],
        }
    );

    agent
        .upload_script_output(
            task_id,
            0,
            futures::stream::iter([Bytes::from_static(b"pc-"), Bytes::from_static(b"42\n")]),
        )
        .await
        .expect("upload script output");
    agent
        .finish_script(task_id, 0, Some(0))
        .await
        .expect("finish script");

    let runs = s
        .container
        .task_repo
        .get_script_runs(task_id, host.id)
        .await
        .expect("script runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].position, 0);
    assert_eq!(runs[0].script_name, script.name);
    assert_eq!(runs[0].output, "pc-42\n");
    assert_eq!(runs[0].exit_code, Some(0));
    assert!(runs[0].finished_at.is_some());

    agent.mark_finished(task_id).await.expect("mark finished");
    let task = s.container.task_repo.get(task_id).await.unwrap();
    assert_eq!(host_state(&task, host.id), TaskState::Done);
    assert_eq!(task.aggregate_state(), TaskState::Done);
}

#[tokio::test]
async fn a_call_for_a_task_that_is_not_next_surfaces_the_servers_message() {
    let s = harness::server().await;
    let (agent, mac) = agent(s);
    let host = seed::host(s, &mac, DISK).await;
    let image_id = seed::image_with_data(
        s,
        &harness::unique_name("rc-errors"),
        b"parttable",
        &[(1, "ext4", &[5u8; 512])],
    )
    .await;
    let current = seed::deploy_task(s, host.id, image_id).await;
    s.container
        .task_repo
        .start(current, host.id)
        .await
        .expect("start the current task");
    let queued = seed::deploy_task(s, host.id, image_id).await;

    let err = agent
        .mark_finished(queued)
        .await
        .expect_err("finishing a queued task is refused");
    let message = err.to_string();
    assert!(
        message.contains(&format!(
            "Task {queued} is not the next task for host {mac}"
        )),
        "{message}"
    );
    assert!(message.contains("400"), "{message}");

    let err = agent
        .partitions(queued)
        .await
        .expect_err("listing a queued task's partitions is refused");
    assert!(
        err.to_string()
            .contains(&format!("No restore task for task {queued} found")),
        "{err}"
    );

    let err = agent
        .upload_partition_table(current, Bytes::from_static(b"table"))
        .await
        .expect_err("capturing into a deploy task is refused");
    assert!(
        err.to_string()
            .contains(&format!("No capture task for task {current} found")),
        "{err}"
    );

    let current_task = s.container.task_repo.get(current).await.unwrap();
    assert_eq!(host_state(&current_task, host.id), TaskState::Running);
    let queued_task = s.container.task_repo.get(queued).await.unwrap();
    assert_eq!(host_state(&queued_task, host.id), TaskState::Pending);
}
