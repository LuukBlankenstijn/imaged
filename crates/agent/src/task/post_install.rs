use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};
use imaged_model::agent::{PostInstall, PostInstallScript};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

use crate::agent::Agent;
use crate::script::{SYSTEM_PATH, script_command, script_env};
use crate::status::ScriptPhase;
use crate::sys::disk::{PartitionTarget, find_target_disk};
use crate::sys::fstab::Matching;
use crate::sys::mount::MountedSystem;

const SCRIPT_PATH: &str = "/tmp/imaged-script";
const OUTPUT_GRACE: Duration = Duration::from_secs(2);
const LIVE_OUTPUT_BYTES: usize = 1024 * 1024;
const KEPT_TAIL_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct Run<'a> {
    root: &'a PartitionTarget,
    first: usize,
    scripts: &'a [PostInstallScript],
}

pub async fn run(
    agent: &Agent,
    task_id: i64,
    restored: &[PartitionTarget],
    matching: Matching,
) -> Result<()> {
    let plan = agent.server.post_install(task_id).await?;
    if plan.scripts.is_empty() {
        return Ok(());
    }
    let runs = runs(&plan.scripts, restored)?;
    let disk = find_target_disk().await?;
    agent.status.update_task(|task| {
        task.scripts = plan
            .scripts
            .iter()
            .map(|script| (script.name.clone(), ScriptPhase::Waiting))
            .collect();
    });
    for run in runs {
        let system = MountedSystem::mount(run.root, &disk, matching)?;
        let ran = run_scripts(agent, task_id, &system, &plan, &run).await;
        let unmounted = system.unmount();
        ran?;
        unmounted?;
    }
    Ok(())
}

fn runs<'a>(
    scripts: &'a [PostInstallScript],
    restored: &'a [PartitionTarget],
) -> Result<Vec<Run<'a>>> {
    let mut first = 0;
    scripts
        .chunk_by(|a, b| a.root_partition == b.root_partition)
        .map(|scripts| {
            let number = scripts[0].root_partition;
            let root = restored
                .iter()
                .find(|partition| partition.number == number)
                .with_context(|| format!("root partition {number} was not restored"))?;
            let run = Run {
                root,
                first,
                scripts,
            };
            first += scripts.len();
            Ok(run)
        })
        .collect()
}

async fn run_scripts(
    agent: &Agent,
    task_id: i64,
    system: &MountedSystem,
    plan: &PostInstall,
    run: &Run<'_>,
) -> Result<()> {
    let identity = agent.status.snapshot().identity;
    for (index, script) in (run.first..).zip(run.scripts) {
        let env = script_env(
            SYSTEM_PATH,
            &script.inputs,
            &identity,
            plan.proxy.as_deref(),
            &plan.variables,
        );
        agent
            .status
            .update_task(|task| task.scripts[index].1 = ScriptPhase::Running);
        let status = run_script(agent, task_id, system.root(), script, env).await?;
        let exited = ScriptPhase::Exited(status.code().map(i64::from));
        agent
            .status
            .update_task(|task| task.scripts[index].1 = exited);
        if !status.success() {
            bail!("post-install script `{}` failed: {status}", script.name);
        }
    }
    Ok(())
}

async fn run_script(
    agent: &Agent,
    task_id: i64,
    root: &Path,
    script: &PostInstallScript,
    env: Vec<(String, String)>,
) -> Result<ExitStatus> {
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
    let result = execute(agent, task_id, root, script, env).await;
    if let Err(e) = std::fs::remove_file(&file) {
        warn!(file=%file.display(), error=%e, "could not remove the script");
    }
    result
}

async fn execute(
    agent: &Agent,
    task_id: i64,
    root: &Path,
    script: &PostInstallScript,
    env: Vec<(String, String)>,
) -> Result<ExitStatus> {
    let (sender, receiver) = tokio::net::unix::pipe::pipe()?;
    let output = sender.into_blocking_fd()?;
    let mut command = Command::new("chroot");
    command
        .arg(root)
        .args(script_command(&script.body, SCRIPT_PATH))
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .kill_on_drop(true);
    let mut child = command.spawn().context("starting chroot")?;
    drop(command);

    let (exited, on_exit) = tokio::sync::oneshot::channel::<()>();
    let lines = bounded(
        output_lines(BufReader::new(receiver), script.name.clone()).take_until(async {
            let _ = on_exit.await;
            tokio::time::sleep(OUTPUT_GRACE).await;
        }),
    );
    let upload = agent
        .server
        .upload_script_output(task_id, script.position, &script.name, lines);
    let wait = async {
        let status = child.wait().await;
        let _ = exited.send(());
        status
    };
    let (uploaded, status) = tokio::join!(upload, wait);
    let status = status.context("waiting for the script")?;
    uploaded.context("sending the script output")?;
    agent
        .server
        .finish_script(task_id, script.position, status.code().map(i64::from))
        .await?;
    info!(script=%script.name, %status, "post-install script finished");
    Ok(status)
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

struct BoundedOutput<S> {
    output: Pin<Box<S>>,
    live: bool,
    sent: usize,
    tail: VecDeque<Bytes>,
    tail_bytes: usize,
    omitted: usize,
}

fn bounded(
    output: impl Stream<Item = Bytes> + Send + 'static,
) -> impl Stream<Item = Bytes> + Send + 'static {
    let state = BoundedOutput {
        output: Box::pin(output),
        live: true,
        sent: 0,
        tail: VecDeque::new(),
        tail_bytes: 0,
        omitted: 0,
    };
    futures::stream::unfold(Some(state), |state| async move {
        let mut state = state?;
        while let Some(chunk) = state.output.next().await {
            state.live &= state.sent + chunk.len() <= LIVE_OUTPUT_BYTES;
            if state.live {
                state.sent += chunk.len();
                return Some((chunk, Some(state)));
            }
            state.tail_bytes += chunk.len();
            state.tail.push_back(chunk);
            while state.tail_bytes > KEPT_TAIL_BYTES {
                let dropped = state.tail.pop_front().expect("a non-empty tail");
                state.tail_bytes -= dropped.len();
                state.omitted += dropped.len();
            }
        }
        if state.tail.is_empty() && state.omitted == 0 {
            return None;
        }
        let mut rest = BytesMut::new();
        if state.omitted > 0 {
            let marker = format!(
                "[… {} bytes of output omitted by the agent …]\n",
                state.omitted
            );
            rest.extend_from_slice(marker.as_bytes());
        }
        for chunk in state.tail {
            rest.extend_from_slice(&chunk);
        }
        Some((rest.freeze(), None))
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
            root_partition: 1,
        }
    }

    fn rooted(position: i64, root_partition: i64) -> PostInstallScript {
        PostInstallScript {
            position,
            root_partition,
            ..script(&[])
        }
    }

    fn restored(number: i64) -> PartitionTarget {
        PartitionTarget {
            device: format!("/dev/vda{number}"),
            number,
            filesystem: imaged_disk::Filesystem::Ext4,
            size: 1,
            fs_uuid: None,
            fs_label: None,
        }
    }

    #[test]
    fn consecutive_scripts_on_the_same_root_share_one_mount_in_script_order() {
        let scripts = [rooted(0, 2), rooted(1, 2), rooted(2, 3), rooted(3, 2)];
        let partitions = [restored(1), restored(2), restored(3)];

        let runs: Vec<_> = runs(&scripts, &partitions)
            .unwrap()
            .into_iter()
            .map(|run| {
                let positions: Vec<i64> = run.scripts.iter().map(|s| s.position).collect();
                (run.root.device.as_str(), run.first, positions)
            })
            .collect();

        assert_eq!(
            runs,
            [
                ("/dev/vda2", 0, vec![0, 1]),
                ("/dev/vda3", 2, vec![2]),
                ("/dev/vda2", 3, vec![3]),
            ]
        );
    }

    #[test]
    fn a_script_rooted_on_a_partition_that_was_not_restored_stops_every_run() {
        let scripts = [rooted(0, 1), rooted(1, 5)];

        let e = runs(&scripts, &[restored(1)]).unwrap_err();

        assert_eq!(e.to_string(), "root partition 5 was not restored");
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

    #[tokio::test]
    async fn output_past_the_live_limit_is_uploaded_only_as_its_tail_after_a_marker() {
        let line = |n: usize| Bytes::from(format!("{n:0>1023}\n"));
        let total_lines = 2 * LIVE_OUTPUT_BYTES / 1024;
        let output = futures::stream::iter((0..total_lines).map(line));

        let uploaded: Vec<Bytes> = bounded(output).collect().await;

        let live_lines = LIVE_OUTPUT_BYTES / 1024;
        let tail_lines = KEPT_TAIL_BYTES / 1024;
        assert_eq!(uploaded.len(), live_lines + 1);
        assert_eq!(
            uploaded[..live_lines],
            (0..live_lines).map(line).collect::<Vec<_>>()
        );
        let omitted = (total_lines - live_lines - tail_lines) * 1024;
        let mut expected_tail =
            format!("[… {omitted} bytes of output omitted by the agent …]\n").into_bytes();
        for n in total_lines - tail_lines..total_lines {
            expected_tail.extend_from_slice(&line(n));
        }
        assert_eq!(uploaded[live_lines], expected_tail);
    }

    #[tokio::test]
    async fn output_within_the_live_limit_is_uploaded_unchanged() {
        let output = [b"one\n".as_slice(), b"two\n"].map(Bytes::from_static);

        let uploaded: Vec<Bytes> = bounded(futures::stream::iter(output.clone()))
            .collect()
            .await;

        assert_eq!(uploaded, output);
    }
}
