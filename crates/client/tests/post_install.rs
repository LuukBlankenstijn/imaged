mod common;

use api::model::{PostInstall, PostInstallScript};
use common::{StubConfig, init_transport, stub_server};

use imaged_client::sys::disk::PartitionTarget;
use imaged_client::task::{ClientTaskExt, DeployTask, MulticastTask};

fn restored(number: i64) -> PartitionTarget {
    PartitionTarget {
        device: format!("/dev/fake{number}"),
        number,
        fstype: "ext4".to_string(),
        size: 0,
    }
}

fn plan(root_partition: Option<i64>) -> PostInstall {
    PostInstall {
        root_partition,
        proxy: None,
        scripts: vec![PostInstallScript {
            position: 0,
            name: "setup".to_string(),
            body: "echo hi".to_string(),
            inputs: Default::default(),
        }],
    }
}

#[tokio::test]
async fn post_install_needs_scripts_and_a_restored_root_before_touching_the_disk() {
    let server = stub_server(
        StubConfig::new()
            .with_post_install(2, plan(None))
            .with_post_install(3, plan(Some(5))),
    )
    .await;
    init_transport(&server.base_url, "aa:bb:cc:dd:ee:ff");
    let partitions = [restored(1), restored(2)];

    DeployTask::new(1)
        .after_partitions(&partitions)
        .await
        .expect("an image without scripts has nothing to run");
    MulticastTask::new(4)
        .after_partitions(&partitions)
        .await
        .expect("a multicast of an image without scripts has nothing to run");

    let no_root = DeployTask::new(2)
        .after_partitions(&partitions)
        .await
        .unwrap_err();
    assert!(
        format!("{no_root:#}").contains("no root partition"),
        "{no_root:#}"
    );

    let missing = MulticastTask::new(3)
        .after_partitions(&partitions)
        .await
        .unwrap_err();
    assert!(
        format!("{missing:#}").contains("root partition 5 was not restored"),
        "{missing:#}"
    );

    let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        [1, 4, 2, 3].map(|task| format!("/api/client/tasks/{task}/scripts"))
    );
}
