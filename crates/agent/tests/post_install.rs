mod common;

use common::{StubConfig, agent_server, stub_server};

use imaged_agent::sys::disk::PartitionTarget;
use imaged_agent::sys::fstab::Matching;
use imaged_agent::task::post_install;
use imaged_model::agent::{PostInstall, PostInstallScript, path};

fn restored(number: i64) -> PartitionTarget {
    PartitionTarget {
        device: format!("/dev/fake{number}"),
        number,
        filesystem: imaged_disk::Filesystem::Ext4,
        size: 0,
        fs_uuid: None,
        fs_label: None,
    }
}

fn plan(roots: &[i64]) -> PostInstall {
    PostInstall {
        proxy: None,
        variables: Default::default(),
        scripts: roots
            .iter()
            .zip(0..)
            .map(|(&root_partition, position)| PostInstallScript {
                position,
                name: format!("setup-{position}"),
                body: "echo hi".to_string(),
                inputs: Default::default(),
                root_partition,
            })
            .collect(),
    }
}

#[tokio::test]
async fn post_install_needs_scripts_and_restored_roots_before_touching_the_disk() {
    let server = stub_server(StubConfig::new().with_post_install(3, plan(&[1, 5, 1]))).await;
    let agent = agent_server(&server.base_url, "aa:bb:cc:dd:ee:ff");
    let partitions = [restored(1), restored(2)];

    post_install::run(&agent, 1, &partitions, Matching::First)
        .await
        .expect("an image without scripts has nothing to run");

    let missing = post_install::run(&agent, 3, &partitions, Matching::Unique)
        .await
        .unwrap_err();
    assert_eq!(missing.to_string(), "root partition 5 was not restored");

    let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, [1, 3].map(path::scripts));
}
