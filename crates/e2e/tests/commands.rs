use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use futures::StreamExt;
use reqwest::{StatusCode, Url};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use imaged_agent::command;
use imaged_agent::server::{Server, Uplink};
use imaged_agent::status::Identity;
use imaged_agent::sys::disk::HostDisk;
use imaged_core::domain::script::NewScript;
use imaged_e2e::harness::{self, TestServer, seed};
use imaged_model::ServerEvent;
use imaged_model::command::{CommandOutcome, CommandScript, HostCommandResult};
use imaged_model::script::{InputKind, ScriptInput, ScriptKind};

const AGENT_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 11));
const WAIT: Duration = Duration::from_secs(10);
const SILENCE_LIMIT: Duration = Duration::from_secs(30);

struct ConnectedAgent {
    host_id: i64,
    mac: String,
    events: std::pin::Pin<Box<dyn futures::Stream<Item = ServerEvent> + Send>>,
    reports: mpsc::UnboundedSender<imaged_model::command::CommandReport>,
}

async fn connected_agent(s: &TestServer) -> ConnectedAgent {
    let mac = harness::unique_mac();
    let base = Url::parse(&s.base_url).expect("harness base url");
    let agent =
        Server::new(base, mac.parse().expect("mac parses"), Some(AGENT_IP)).expect("agent client");
    let (reports, commands) = mpsc::unbounded_channel();
    let uplink = Uplink {
        progress: watch::channel(None).1,
        idle: watch::channel(()).1,
        commands,
    };
    let disk = Ok(HostDisk {
        size_bytes: 1 << 30,
        partitions: Vec::new(),
    });
    let mut events = Box::pin(
        agent
            .connect_events(&disk, uplink, SILENCE_LIMIT)
            .await
            .expect("connect events"),
    );
    assert_eq!(
        tokio::time::timeout(WAIT, events.next()).await.unwrap(),
        Some(ServerEvent::Current(None))
    );
    let host = s
        .container
        .host_repo
        .get_by_mac(&mac)
        .await
        .expect("host registered");
    ConnectedAgent {
        host_id: host.id,
        mac,
        events,
        reports,
    }
}

fn answering(agent: ConnectedAgent, server: &str) -> JoinHandle<()> {
    let identity = Identity {
        mac: agent.mac,
        ip: Some(AGENT_IP.to_string()),
        server: server.to_string(),
    };
    let ConnectedAgent {
        mut events,
        reports,
        ..
    } = agent;
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            if let ServerEvent::Run(request) = event {
                command::spawn(request, identity.clone(), reports.clone());
            }
        }
    })
}

async fn run(s: &TestServer, body: Value) -> reqwest::Response {
    tokio::time::timeout(WAIT, s.ui_post("/api/ui/hosts/command", &body))
        .await
        .expect("an answer within the timeout")
}

#[tokio::test]
async fn a_command_runs_on_every_connected_agent_at_once_and_every_host_gets_its_own_answer() {
    let s = harness::server().await;
    let first = connected_agent(s).await;
    let second = connected_agent(s).await;
    let hosts = [
        (first.host_id, first.mac.clone()),
        (second.host_id, second.mac.clone()),
    ];
    let offline = seed::host(s, &harness::unique_mac(), 1024).await;
    let answering = [
        answering(first, &s.base_url),
        answering(second, &s.base_url),
    ];

    let started = std::time::Instant::now();
    let response = run(
        s,
        json!({
            "host_ids": [hosts[0].0, offline.id, hosts[1].0, hosts[0].0],
            "run": { "command": "sleep 2; echo \"$IMAGED_MAC\"; echo oops >&2; exit 4" },
        }),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(response.status(), StatusCode::OK);
    let results: Vec<HostCommandResult> = response.json().await.expect("results decode");

    let exited = |(host_id, mac): &(i64, String)| HostCommandResult {
        host_id: *host_id,
        outcome: CommandOutcome::Exited {
            code: Some(4),
            output: format!("{mac}\noops\n"),
        },
    };
    assert_eq!(
        results,
        vec![
            exited(&hosts[0]),
            HostCommandResult {
                host_id: offline.id,
                outcome: CommandOutcome::Failed {
                    reason: "the host is not connected".into(),
                },
            },
            exited(&hosts[1]),
        ]
    );
    assert!(elapsed < Duration::from_millis(3500), "{elapsed:?}");
    for agent in answering {
        agent.abort();
    }
}

#[tokio::test]
async fn a_client_script_runs_with_its_inputs_and_a_post_install_script_is_refused() {
    let s = harness::server().await;
    let agent = connected_agent(s).await;
    let host_id = agent.host_id;
    let mac = agent.mac.clone();
    let answering = answering(agent, &s.base_url);
    let greet = s
        .container
        .script_repo
        .create(&NewScript {
            name: harness::unique_name("greet"),
            kind: ScriptKind::Client,
            body: "#!/bin/sh\necho \"$IMAGED_INPUT_GREETING from $IMAGED_MAC\"\n".into(),
            inputs: vec![ScriptInput {
                name: "greeting".into(),
                label: "Greeting".into(),
                description: None,
                kind: InputKind::Text,
                required: true,
                default_value: None,
                per_machine: true,
                choices: Vec::new(),
            }],
        })
        .await
        .expect("create client script");
    let setup = s
        .container
        .script_repo
        .create(&NewScript {
            name: harness::unique_name("setup"),
            kind: ScriptKind::PostInstall,
            body: "echo".into(),
            inputs: Vec::new(),
        })
        .await
        .expect("create post-install script");

    let response = run(
        s,
        json!({
            "host_ids": [host_id],
            "run": { "script": {
                "id": greet.id,
                "inputs": { "per_machine": { host_id.to_string(): { "greeting": { "value": "hi" } } } },
            } },
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let results: Vec<HostCommandResult> = response.json().await.expect("results decode");
    assert_eq!(
        results,
        vec![HostCommandResult {
            host_id,
            outcome: CommandOutcome::Exited {
                code: Some(0),
                output: format!("hi from {mac}\n"),
            },
        }]
    );

    let missing_input = run(
        s,
        json!({ "host_ids": [host_id], "run": { "script": { "id": greet.id } } }),
    )
    .await;
    assert_eq!(missing_input.status(), StatusCode::BAD_REQUEST);
    let post_install = run(
        s,
        json!({ "host_ids": [host_id], "run": { "script": { "id": setup.id } } }),
    )
    .await;
    assert_eq!(post_install.status(), StatusCode::BAD_REQUEST);
    answering.abort();
}

#[tokio::test]
async fn an_agent_that_disconnects_before_answering_fails_its_command() {
    let s = harness::server().await;
    let ConnectedAgent {
        host_id,
        mut events,
        ..
    } = connected_agent(s).await;
    let agent = tokio::spawn(async move {
        while let Some(event) = events.next().await {
            if let ServerEvent::Run(_) = event {
                return;
            }
        }
    });

    let script = CommandScript {
        body: "sleep 60".into(),
        ..CommandScript::default()
    };
    let results =
        tokio::time::timeout(WAIT, s.container.host_registry.run(vec![(host_id, script)]))
            .await
            .expect("an answer within the timeout");

    assert_eq!(
        results,
        vec![HostCommandResult {
            host_id,
            outcome: CommandOutcome::Failed {
                reason: "the host disconnected before answering".into(),
            },
        }]
    );
    agent.await.unwrap();
}
