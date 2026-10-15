use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer};

const KEPT_LINES: usize = 1000;
const LOGGED_CRATES: [&str; 3] = ["imaged-agent", "imaged-disk", "scuttlecast"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub level: Level,
    pub text: String,
}

#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<VecDeque<LogLine>>>);

impl LogBuffer {
    pub fn lines(&self) -> Vec<LogLine> {
        let lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        lines.iter().cloned().collect()
    }

    fn push(&self, line: LogLine) {
        let mut lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if lines.len() == KEPT_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

impl<S: Subscriber> Layer<S> for LogBuffer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut text = clock();
        event.record(&mut Fields(&mut text));
        self.push(LogLine {
            level: *event.metadata().level(),
            text,
        });
    }
}

struct Fields<'a>(&'a mut String);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = match field.name() {
            "message" => write!(self.0, " {value:?}"),
            name => write!(self.0, " {name}={value:?}"),
        };
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        let _ = match field.name() {
            "message" => write!(self.0, " {value}"),
            name => write!(self.0, " {name}={value}"),
        };
    }
}

fn clock() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

pub fn init(level: &str, screen_is_taken: bool) -> LogBuffer {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(imaged_model::log_filter(&LOGGED_CRATES, level)));
    let plain = if screen_is_taken {
        serial_console().map(|serial| {
            tracing_subscriber::fmt::layer()
                .with_writer(Mutex::new(serial))
                .boxed()
        })
    } else {
        Some(tracing_subscriber::fmt::layer().boxed())
    };
    let buffer = LogBuffer::default();
    tracing_subscriber::registry()
        .with(filter)
        .with(plain)
        .with(buffer.clone())
        .init();
    buffer
}

pub fn serial_console() -> Option<File> {
    let consoles = std::fs::read_to_string("/sys/class/tty/console/active").ok()?;
    consoles
        .split_whitespace()
        .find(|name| !is_virtual_terminal(name))
        .and_then(|name| {
            OpenOptions::new()
                .write(true)
                .open(format!("/dev/{name}"))
                .ok()
        })
}

fn is_virtual_terminal(name: &str) -> bool {
    name.strip_prefix("tty")
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_ports_are_told_apart_from_virtual_terminals() {
        assert!(is_virtual_terminal("tty0"));
        assert!(is_virtual_terminal("tty12"));
        assert!(!is_virtual_terminal("ttyS0"));
        assert!(!is_virtual_terminal("ttyAMA0"));
        assert!(!is_virtual_terminal("hvc0"));
        assert!(!is_virtual_terminal("tty"));
    }

    #[test]
    fn the_buffer_keeps_the_newest_lines_with_their_level() {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::registry().with(buffer.clone());

        tracing::subscriber::with_default(subscriber, || {
            for n in 0..KEPT_LINES + 5 {
                tracing::warn!(n, "line");
            }
        });

        let lines = buffer.lines();
        assert_eq!(lines.len(), KEPT_LINES);
        assert_eq!(lines[0].level, Level::WARN);
        assert!(lines[0].text.ends_with(" line n=5"), "{}", lines[0].text);
    }
}
