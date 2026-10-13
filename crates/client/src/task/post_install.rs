use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use api::model::{PostInstall, PostInstallScript};
use bytes::Bytes;
use dioxus_fullstack::ByteStream;
use futures::{Stream, StreamExt};
use imaged_model::script_env::{AGENT_VARIABLES, INPUT_PREFIX, PROXY_VARIABLES};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

use crate::sys::disk::PartitionTarget;
use crate::sys::mount::MountedSystem;

const SCRIPT_PATH: &str = "/tmp/imaged-script";
const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

struct Agent {
    mac: String,
    ip: String,
    server: String,
}

impl Agent {
    fn current() -> Result<Self> {
        Ok(Self {
            mac: crate::sys::get_mac()?.to_string().to_ascii_lowercase(),
            ip: crate::sys::get_ip()
                .map(|ip| ip.to_string())
                .unwrap_or_default(),
            server: dioxus_fullstack::get_server_url().to_string(),
        })
    }
}

pub async fn run(task_id: i64, partitions: &[PartitionTarget]) -> Result<()> {
    let plan = api::scripts::post_install(task_id).await?;
    if plan.scripts.is_empty() {
        return Ok(());
    }
    let root = root_partition(&plan, partitions)?;
    let disk = crate::sys::disk::find_target_disk().await?;
    let agent = Agent::current()?;
    let system = MountedSystem::mount(root, &disk)?;
    let ran = run_scripts(task_id, &system, &plan, &agent).await;
    let unmounted = system.unmount();
    ran?;
    unmounted
}

async fn run_scripts(
    task_id: i64,
    system: &MountedSystem,
    plan: &PostInstall,
    agent: &Agent,
) -> Result<()> {
    for script in &plan.scripts {
        let env = script_env(script, agent, plan.proxy.as_deref(), &plan.variables);
        run_script(task_id, system.root(), script, env).await?;
    }
    Ok(())
}

fn root_partition<'a>(
    plan: &PostInstall,
    partitions: &'a [PartitionTarget],
) -> Result<&'a PartitionTarget> {
    let number = plan
        .root_partition
        .context("the image has post-install scripts but no root partition")?;
    partitions
        .iter()
        .find(|partition| partition.number == number)
        .with_context(|| format!("root partition {number} was not restored"))
}

fn script_env(
    script: &PostInstallScript,
    agent: &Agent,
    proxy: Option<&str>,
    variables: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let agent_values = [PATH, "/root", &agent.mac, &agent.ip, &agent.server];
    let mut env: Vec<(String, String)> = AGENT_VARIABLES
        .iter()
        .zip(agent_values)
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    env.extend(
        variables
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    env.extend(script.inputs.iter().map(|(name, value)| {
        (
            format!("{INPUT_PREFIX}{}", name.to_ascii_uppercase()),
            value.clone(),
        )
    }));
    if let Some(proxy) = proxy {
        env.extend(
            PROXY_VARIABLES
                .iter()
                .map(|name| (name.to_string(), proxy.to_string())),
        );
    }
    env
}

fn script_command(body: &str) -> &'static [&'static str] {
    if body.starts_with("#!") {
        &[SCRIPT_PATH]
    } else {
        &["/bin/sh", SCRIPT_PATH]
    }
}

async fn run_script(
    task_id: i64,
    root: &Path,
    script: &PostInstallScript,
    env: Vec<(String, String)>,
) -> Result<()> {
    info!(script=%script.name, "running post-install script");
    let file = root.join(SCRIPT_PATH.trim_start_matches('/'));
    if let Some(dir) = file.parent()
        && !dir.exists()
    {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777))?;
    }
    std::fs::write(&file, &script.body).with_context(|| format!("writing {}", file.display()))?;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700))?;
    let result = execute(task_id, root, script, env).await;
    if let Err(e) = std::fs::remove_file(&file) {
        warn!(file=%file.display(), error=%e, "could not remove the script");
    }
    result
}

async fn execute(
    task_id: i64,
    root: &Path,
    script: &PostInstallScript,
    env: Vec<(String, String)>,
) -> Result<()> {
    let (sender, receiver) = tokio::net::unix::pipe::pipe()?;
    let output = sender.into_blocking_fd()?;
    let mut command = Command::new("chroot");
    command
        .arg(root)
        .args(script_command(&script.body))
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .kill_on_drop(true);
    let mut child = command.spawn().context("starting chroot")?;
    drop(command);

    let (exited, on_exit) = tokio::sync::oneshot::channel::<()>();
    let lines = output_lines(BufReader::new(receiver), script.name.clone()).take_until(async {
        let _ = on_exit.await;
        tokio::time::sleep(OUTPUT_GRACE).await;
    });
    let upload =
        api::scripts::upload_script_output(task_id, script.position, ByteStream::new(lines));
    let wait = async {
        let status = child.wait().await;
        let _ = exited.send(());
        status
    };
    let (uploaded, status) = tokio::join!(upload, wait);
    let status = status.context("waiting for the script")?;
    uploaded.context("sending the script output")?;
    api::scripts::finish_script(task_id, script.position, status.code().map(i64::from)).await?;

    if !status.success() {
        bail!("post-install script `{}` failed: {status}", script.name);
    }
    info!(script=%script.name, "post-install script finished");
    Ok(())
}

fn output_lines(
    reader: impl AsyncBufRead + Unpin + Send + 'static,
    script: String,
) -> impl Stream<Item = Bytes> + Send + 'static {
    futures::stream::unfold(Some(reader), move |reader| {
        let script = script.clone();
        async move {
            let mut reader = reader?;
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) => None,
                Ok(_) => {
                    info!(%script, "{}", String::from_utf8_lossy(&line).trim_end());
                    Some((Bytes::from(line), Some(reader)))
                }
                Err(e) => {
                    warn!(%script, error=%e, "could not read the script output");
                    None
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn script(inputs: &[(&str, &str)]) -> PostInstallScript {
        PostInstallScript {
            position: 0,
            name: "setup".to_string(),
            body: "echo".to_string(),
            inputs: inputs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn agent() -> Agent {
        Agent {
            mac: "52:54:00:12:34:56".to_string(),
            ip: "10.0.2.15".to_string(),
            server: "http://10.0.2.2:8080".to_string(),
        }
    }

    fn lookup<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn inputs_and_the_agent_identity_reach_the_script_as_variables() {
        let env = script_env(
            &script(&[("host_name", "pc-1"), ("site", "")]),
            &agent(),
            None,
            &BTreeMap::new(),
        );

        assert_eq!(lookup(&env, "IMAGED_INPUT_HOST_NAME"), Some("pc-1"));
        assert_eq!(lookup(&env, "IMAGED_INPUT_SITE"), Some(""));
        assert_eq!(lookup(&env, "IMAGED_MAC"), Some("52:54:00:12:34:56"));
        assert_eq!(lookup(&env, "IMAGED_IP"), Some("10.0.2.15"));
        assert_eq!(lookup(&env, "IMAGED_SERVER"), Some("http://10.0.2.2:8080"));
        assert!(lookup(&env, "PATH").is_some_and(|path| path.contains("/usr/bin")));
        assert_eq!(lookup(&env, "http_proxy"), None);
    }

    #[test]
    fn a_configured_proxy_is_exported_in_both_spellings() {
        let env = script_env(
            &script(&[]),
            &agent(),
            Some("http://proxy:3128"),
            &BTreeMap::new(),
        );

        for name in PROXY_VARIABLES {
            assert_eq!(lookup(&env, name), Some("http://proxy:3128"), "{name}");
        }
    }

    #[test]
    fn variables_from_the_server_reach_every_script() {
        let variables = BTreeMap::from([("IMAGED_IMAGE_TOKEN".to_string(), "secret".to_string())]);
        let env = script_env(&script(&[]), &agent(), None, &variables);

        assert_eq!(lookup(&env, "IMAGED_IMAGE_TOKEN"), Some("secret"));
    }

    #[test]
    fn a_shebang_picks_the_interpreter_and_anything_else_runs_in_sh() {
        assert_eq!(
            script_command("#!/usr/bin/env python3\nprint()"),
            [SCRIPT_PATH]
        );
        assert_eq!(script_command("echo hi"), ["/bin/sh", SCRIPT_PATH]);
        assert_eq!(script_command(" #!/bin/bash"), ["/bin/sh", SCRIPT_PATH]);
    }

    #[tokio::test]
    async fn output_is_forwarded_line_by_line_including_an_unterminated_last_line() {
        let output: &[u8] = b"one\ntwo\nthree";
        let lines: Vec<Bytes> = output_lines(output, "setup".to_string()).collect().await;

        assert_eq!(
            lines,
            [b"one\n".as_slice(), b"two\n", b"three"].map(Bytes::from_static)
        );
    }
}
