use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use imaged_model::command::{
    COMMAND_OUTPUT_BYTES, COMMAND_TIMEOUT, CommandOutcome, CommandReport, CommandRequest,
    CommandScript,
};
use rustix::process::{Pid, Signal, kill_process_group};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

use crate::script::{SYSTEM_PATH, script_command, script_env};
use crate::status::Identity;

const OUTPUT_GRACE: Duration = Duration::from_millis(500);

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub fn spawn(
    request: CommandRequest,
    identity: Identity,
    reports: mpsc::UnboundedSender<CommandReport>,
) {
    tokio::spawn(async move {
        let CommandRequest { id, script } = request;
        tracing::info!(id, body = %script.body, "running a command from the server");
        let outcome = run(&script, &identity, COMMAND_TIMEOUT).await;
        match &outcome {
            CommandOutcome::Exited { code, .. } => tracing::info!(id, ?code, "command exited"),
            CommandOutcome::TimedOut { .. } => tracing::warn!(id, "command timed out"),
            CommandOutcome::Failed { reason } => tracing::warn!(id, %reason, "command failed"),
        }
        let _ = reports.send(CommandReport { id, outcome });
    });
}

pub async fn run(script: &CommandScript, identity: &Identity, limit: Duration) -> CommandOutcome {
    let file = std::env::temp_dir().join(format!(
        "imaged-command-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = run_file(script, identity, limit, &file).await;
    if let Err(e) = std::fs::remove_file(&file) {
        tracing::warn!(file = %file.display(), error = %e, "could not remove the command's script");
    }
    outcome
}

async fn run_file(
    script: &CommandScript,
    identity: &Identity,
    limit: Duration,
    file: &Path,
) -> CommandOutcome {
    let (mut child, output) = match start(script, identity, file).await {
        Ok(started) => started,
        Err(e) => {
            return CommandOutcome::Failed {
                reason: format!("could not start the script: {e}"),
            };
        }
    };
    let group = child.id().and_then(|id| Pid::from_raw(id as i32));
    let mut captured = Captured::default();
    match wait(&mut child, output, &mut captured, limit).await {
        Some(Ok(status)) => CommandOutcome::Exited {
            code: status.code(),
            output: captured.into_text(),
        },
        Some(Err(e)) => CommandOutcome::Failed {
            reason: format!("waiting for the script: {e}"),
        },
        None => {
            if let Some(group) = group {
                let _ = kill_process_group(group, Signal::KILL);
            }
            CommandOutcome::TimedOut {
                output: captured.into_text(),
            }
        }
    }
}

async fn start(
    script: &CommandScript,
    identity: &Identity,
    file: &Path,
) -> std::io::Result<(Child, tokio::net::unix::pipe::Receiver)> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, &script.body)?;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700))?;
    let path = file.to_string_lossy();
    let (sender, receiver) = tokio::net::unix::pipe::pipe()?;
    let output = sender.into_blocking_fd()?;
    let argv = script_command(&script.body, &path);
    let search_path = std::env::var("PATH").unwrap_or_else(|_| SYSTEM_PATH.to_string());
    let env = script_env(
        &search_path,
        &script.inputs,
        identity,
        script.proxy.as_deref(),
        &script.variables,
    );
    let mut command = Command::new(argv[0]);
    command
        .args(&argv[1..])
        .env_clear()
        .envs(env)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .process_group(0)
        .kill_on_drop(true);
    let mut busy_retries = 5;
    loop {
        match command.spawn() {
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy && busy_retries > 0 => {
                busy_retries -= 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            spawned => return Ok((spawned?, receiver)),
        }
    }
}

async fn wait(
    child: &mut Child,
    mut output: impl AsyncRead + Unpin,
    captured: &mut Captured,
    limit: Duration,
) -> Option<std::io::Result<ExitStatus>> {
    let (exited, on_exit) = oneshot::channel::<()>();
    let status = async {
        let status = tokio::time::timeout(limit, child.wait()).await.ok();
        let _ = exited.send(());
        status
    };
    let reading = async {
        tokio::select! {
            _ = captured.read_all(&mut output) => {}
            _ = async {
                let _ = on_exit.await;
                tokio::time::sleep(OUTPUT_GRACE).await;
            } => {}
        }
    };
    let (status, ()) = tokio::join!(status, reading);
    status
}

#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    omitted: usize,
}

impl Captured {
    async fn read_all(&mut self, output: &mut (impl AsyncRead + Unpin)) {
        let mut chunk = [0; 8192];
        while let Ok(read @ 1..) = output.read(&mut chunk).await {
            self.bytes.extend_from_slice(&chunk[..read]);
            if self.bytes.len() > 2 * COMMAND_OUTPUT_BYTES {
                self.keep_tail();
            }
        }
    }

    fn keep_tail(&mut self) {
        let excess = self.bytes.len().saturating_sub(COMMAND_OUTPUT_BYTES);
        self.bytes.drain(..excess);
        self.omitted += excess;
    }

    fn into_text(mut self) -> String {
        self.keep_tail();
        let output = String::from_utf8_lossy(&self.bytes);
        match self.omitted {
            0 => output.into_owned(),
            omitted => format!("[… {omitted} bytes of output omitted by the agent …]\n{output}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Instant;

    use super::*;

    const LIMIT: Duration = Duration::from_secs(5);

    fn identity() -> Identity {
        Identity {
            mac: "52:54:00:12:34:56".to_string(),
            ip: None,
            server: "http://10.0.2.2:8080".to_string(),
        }
    }

    async fn shell(body: &str, limit: Duration) -> CommandOutcome {
        let script = CommandScript {
            body: body.to_string(),
            ..CommandScript::default()
        };
        run(&script, &identity(), limit).await
    }

    #[tokio::test]
    async fn the_exit_code_and_both_output_streams_come_back_in_order() {
        let outcome = shell("echo out; echo err >&2; echo again; exit 3", LIMIT).await;
        assert_eq!(
            outcome,
            CommandOutcome::Exited {
                code: Some(3),
                output: "out\nerr\nagain\n".into(),
            }
        );
    }

    #[tokio::test]
    async fn a_script_runs_under_its_shebang_with_its_inputs_and_the_agent_identity() {
        let script = CommandScript {
            body: "#!/bin/sh -e\necho \"$IMAGED_INPUT_GREETING $IMAGED_MAC $0\"\nfalse\necho unreachable\n"
                .to_string(),
            inputs: BTreeMap::from([("greeting".to_string(), "hello".to_string())]),
            ..CommandScript::default()
        };

        let CommandOutcome::Exited { code, output } = run(&script, &identity(), LIMIT).await else {
            panic!("the script must run to its end");
        };

        assert_eq!(code, Some(1));
        assert!(
            output.starts_with("hello 52:54:00:12:34:56 ")
                && output.contains("imaged-command-")
                && !output.contains("unreachable"),
            "{output}"
        );
    }

    #[tokio::test]
    async fn a_command_running_past_the_limit_is_killed_with_the_output_so_far() {
        let started = Instant::now();
        let outcome = shell("echo begun; sleep 30", Duration::from_secs(1)).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            outcome,
            CommandOutcome::TimedOut {
                output: "begun\n".into(),
            }
        );
    }

    #[tokio::test]
    async fn a_background_process_holding_the_output_open_does_not_hold_up_the_answer() {
        let started = Instant::now();
        let outcome = shell("sleep 3 & echo started", LIMIT).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            outcome,
            CommandOutcome::Exited {
                code: Some(0),
                output: "started\n".into(),
            }
        );
    }

    #[tokio::test]
    async fn long_output_keeps_its_end_and_says_how_much_was_left_out() {
        let total = 3 * COMMAND_OUTPUT_BYTES;
        let outcome = shell(
            &format!("head -c {total} /dev/zero | tr '\\0' a; printf END"),
            LIMIT,
        )
        .await;
        let CommandOutcome::Exited {
            code: Some(0),
            output,
        } = outcome
        else {
            panic!("unexpected outcome {outcome:?}");
        };
        let omitted = total + 3 - COMMAND_OUTPUT_BYTES;
        let marker = format!("[… {omitted} bytes of output omitted by the agent …]\n");
        assert!(output.starts_with(&marker));
        assert!(output.ends_with("aaaEND"));
        assert_eq!(output.len(), marker.len() + COMMAND_OUTPUT_BYTES);
    }
}
