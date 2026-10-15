mod view;

use std::fs::File;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::time::Instant;

use crate::agent::Agent;
use crate::logging::LogBuffer;
use crate::task::ClientState;

const REDRAW_INTERVAL: Duration = Duration::from_millis(250);
const SHELL_VT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Progress,
    Logs,
}

#[derive(Debug)]
pub struct App {
    pub tab: Tab,
    pub confirming_reboot: bool,
    pub log_scroll: usize,
    pub shell: bool,
}

enum Action {
    OpenShell,
    Reboot,
}

pub async fn run(agent: Agent, client: Arc<ClientState>, logs: LogBuffer) -> io::Result<()> {
    let mut terminal = take_screen()?;
    let mut app = App {
        tab: Tab::Progress,
        confirming_reboot: false,
        log_scroll: 0,
        shell: on_virtual_terminal(),
    };
    let mut keys = EventStream::new();
    let mut redraw = tokio::time::interval(REDRAW_INTERVAL);
    let result = loop {
        tokio::select! {
            _ = redraw.tick() => {}
            event = keys.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    agent.status.cancel_reboot();
                    match handle_key(&mut app, key) {
                        Some(Action::OpenShell) => {
                            if let Err(e) = switch_virtual_terminal(SHELL_VT) {
                                tracing::warn!(error=%e, "could not switch to the shell");
                            }
                        }
                        Some(Action::Reboot) => {
                            let (agent, client) = (agent.clone(), client.clone());
                            tokio::spawn(async move {
                                if let Err(e) = crate::reboot::from_console(&agent, &client).await {
                                    tracing::error!(error=%format_args!("{e:#}"), "could not reboot");
                                }
                            });
                        }
                        None => {}
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e),
                None => break Ok(()),
            },
        }
        let state = agent.status.snapshot();
        let lines = logs.lines();
        if let Err(e) =
            terminal.draw(|frame| view::draw(frame, &app, &state, &lines, Instant::now()))
        {
            break Err(e);
        }
    };
    ratatui::try_restore()?;
    result
}

fn take_screen() -> io::Result<DefaultTerminal> {
    let off_screen = match crate::logging::serial_console() {
        Some(serial) => serial,
        None => File::options().write(true).open("/dev/null")?,
    };
    rustix::stdio::dup2_stderr(&off_screen)?;
    ratatui::try_init()
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<Action> {
    if app.confirming_reboot {
        app.confirming_reboot = false;
        return matches!(key.code, KeyCode::Char('y' | 'Y')).then_some(Action::Reboot);
    }
    match key.code {
        KeyCode::F(1) | KeyCode::Char('1') => app.tab = Tab::Progress,
        KeyCode::F(2) | KeyCode::Char('2') => app.tab = Tab::Logs,
        KeyCode::Tab => {
            app.tab = match app.tab {
                Tab::Progress => Tab::Logs,
                Tab::Logs => Tab::Progress,
            }
        }
        KeyCode::F(3) | KeyCode::Char('3') if app.shell => return Some(Action::OpenShell),
        KeyCode::F(10) => app.confirming_reboot = true,
        KeyCode::PageUp => app.log_scroll += 10,
        KeyCode::PageDown => app.log_scroll = app.log_scroll.saturating_sub(10),
        KeyCode::Up => app.log_scroll += 1,
        KeyCode::Down => app.log_scroll = app.log_scroll.saturating_sub(1),
        KeyCode::End => app.log_scroll = 0,
        _ => {}
    }
    None
}

fn on_virtual_terminal() -> bool {
    rustix::termios::ttyname(rustix::stdio::stdin(), Vec::new()).is_ok_and(|name| {
        name.to_bytes()
            .strip_prefix(b"/dev/tty")
            .is_some_and(|number| !number.is_empty() && number.iter().all(u8::is_ascii_digit))
    })
}

fn switch_virtual_terminal(number: usize) -> io::Result<()> {
    const VT_ACTIVATE: rustix::ioctl::Opcode = 0x5606;
    // SAFETY: VT_ACTIVATE takes the terminal number by value, not a pointer.
    unsafe {
        let activate = rustix::ioctl::IntegerSetter::<VT_ACTIVATE>::new_usize(number);
        rustix::ioctl::ioctl(rustix::stdio::stdin(), activate)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App {
            tab: Tab::Progress,
            confirming_reboot: false,
            log_scroll: 0,
            shell: true,
        }
    }

    fn press(app: &mut App, code: KeyCode) -> Option<Action> {
        handle_key(app, KeyEvent::from(code))
    }

    #[test]
    fn function_keys_switch_tabs_and_open_the_shell() {
        let mut app = app();

        press(&mut app, KeyCode::F(2));
        assert_eq!(app.tab, Tab::Logs);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, Tab::Progress);
        assert!(matches!(
            press(&mut app, KeyCode::F(3)),
            Some(Action::OpenShell)
        ));
    }

    #[test]
    fn the_shell_is_only_offered_on_a_virtual_terminal() {
        let mut app = App {
            shell: false,
            ..app()
        };

        assert!(press(&mut app, KeyCode::F(3)).is_none());
    }

    #[test]
    fn a_reboot_needs_confirmation_and_anything_but_yes_backs_out() {
        let mut app = app();

        assert!(press(&mut app, KeyCode::F(10)).is_none());
        assert!(app.confirming_reboot);
        assert!(press(&mut app, KeyCode::Char('n')).is_none());
        assert!(!app.confirming_reboot);

        press(&mut app, KeyCode::F(10));
        assert!(matches!(
            press(&mut app, KeyCode::Char('y')),
            Some(Action::Reboot)
        ));
    }

    #[test]
    fn scrolling_the_logs_stops_at_the_newest_line() {
        let mut app = app();

        press(&mut app, KeyCode::PageUp);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.log_scroll, 9);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.log_scroll, 0);
    }
}
