use std::io::Read;
use std::process::Command;
use tokio::task;

pub async fn watch_for_shell_hotkey() {
    task::spawn_blocking(|| {
        if let Err(e) = run_watcher() {
            tracing::error!(err=%e, "shell hotkey watcher died");
        }
    })
    .await
    .ok();
}

fn run_watcher() -> std::io::Result<()> {
    configure_stdin().map_err(std::io::Error::from)?;

    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let mut buf = [0u8; 1];

    loop {
        if stdin.read(&mut buf)? == 0 {
            continue;
        }
        if buf[0] == 0x13 {
            tracing::info!("ctrl-s pressed, dropping into shell");
            match Command::new("/bin/sh").status() {
                Ok(s) => tracing::info!(?s, "shell exited"),
                Err(e) => tracing::error!(err=%e, "failed to spawn shell"),
            }
        }
    }
}

fn configure_stdin() -> rustix::io::Result<()> {
    use rustix::termios::{
        InputModes, LocalModes, OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr,
    };

    let stdin = rustix::stdio::stdin();
    let mut t = tcgetattr(stdin)?;
    t.input_modes.remove(InputModes::IXON);
    t.local_modes.remove(LocalModes::ICANON);
    t.special_codes[SpecialCodeIndex::VMIN] = 1;
    t.special_codes[SpecialCodeIndex::VTIME] = 0;
    tcsetattr(stdin, OptionalActions::Now, &t)?;
    Ok(())
}
