mod multicast;
mod task;

use std::time::Duration;

use imaged_model::TaskType;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use tokio::time::Instant;
use tracing::Level;
use tui_big_text::{BigText, PixelSize};

use super::{App, Tab};
use crate::logging::LogLine;
use crate::status::{AgentState, Connection, TaskState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Good,
    Warning,
    Bad,
    Idle,
}

impl Health {
    pub fn color(self) -> Color {
        match self {
            Health::Good => Color::Green,
            Health::Warning => Color::Yellow,
            Health::Bad => Color::Red,
            Health::Idle => Color::Blue,
        }
    }
}

pub fn draw(frame: &mut Frame, app: &App, state: &AgentState, logs: &[LogLine], now: Instant) {
    let [header, body] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(frame.area());
    draw_header(frame, header, app, state);
    match (app.tab, &state.task) {
        (Tab::Logs, _) => draw_logs(frame, body, logs, app.log_scroll),
        (Tab::Progress, None) => draw_idle(frame, body, state, logs),
        (Tab::Progress, Some(task)) => match &task.multicast {
            Some(multicast) => {
                multicast::draw(frame, body, task, multicast, state.reboot, logs, now)
            }
            None => task::draw(frame, body, task, state.reboot, logs, now),
        },
    }
    if app.confirming_reboot {
        draw_reboot_dialog(frame, state.running_task().is_some());
    }
}

pub fn draw_big_or_plain(frame: &mut Frame, area: Rect, text: &str, style: Style) {
    let width = text.chars().count() as u16;
    let size = [
        (PixelSize::Full, 8, 8),
        (PixelSize::HalfHeight, 4, 8),
        (PixelSize::HalfWidth, 8, 4),
    ]
    .into_iter()
    .find(|&(_, rows, columns)| area.height >= rows && area.width >= width * columns);
    let [centered] = Layout::vertical([Constraint::Length(size.map_or(1, |(_, rows, _)| rows))])
        .flex(Flex::Center)
        .areas(area);
    match size {
        Some((size, _, _)) => frame.render_widget(
            BigText::builder()
                .pixel_size(size)
                .style(style)
                .alignment(Alignment::Center)
                .lines(vec![Line::from(text.to_string())])
                .build(),
            centered,
        ),
        None => frame.render_widget(
            Paragraph::new(Line::styled(
                text.to_string(),
                style.add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
            centered,
        ),
    }
}

pub fn format_percent(fraction: f64) -> String {
    format!("{:.0}%", (fraction * 100.0).floor())
}

pub fn format_bytes(bytes: f64) -> String {
    let (value, unit) = [(1e12, "TB"), (1e9, "GB"), (1e6, "MB"), (1e3, "KB")]
        .into_iter()
        .find(|&(scale, _)| bytes >= scale)
        .map_or((bytes, "B"), |(scale, unit)| (bytes / scale, unit));
    if value >= 100.0 || unit == "B" {
        format!("{value:.0} {unit}")
    } else {
        format!("{value:.1} {unit}")
    }
}

pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, minutes, seconds) => format!("{minutes}:{seconds:02}"),
        (hours, minutes, seconds) => format!("{hours}:{minutes:02}:{seconds:02}"),
    }
}

pub fn draw_task_title(frame: &mut Frame, area: Rect, task: &TaskState, now: Instant) {
    let kind = match task.kind {
        TaskType::Capture => "CAPTURE",
        TaskType::CapturePartition => "CAPTURE PARTITION",
        TaskType::Deploy => "DEPLOY",
        TaskType::DeployPartition => "DEPLOY PARTITION",
        TaskType::Multicast => "MULTICAST",
        TaskType::MulticastPartition => "MULTICAST PARTITION",
        TaskType::Reboot => "REBOOT",
    };
    let image = task
        .image_id
        .map_or(String::new(), |id| format!(" · image {id}"));
    let elapsed = task
        .outcome
        .as_ref()
        .map_or_else(|| now.duration_since(task.started), |(_, took)| *took);
    frame.render_widget(
        Line::from(vec![
            Span::styled(
                format!(" {kind} "),
                Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
            ),
            Span::raw(format!(
                " task {}{image} · {}",
                task.id,
                format_duration(elapsed)
            )),
        ]),
        area,
    );
}

pub fn log_line(line: &LogLine) -> Line<'static> {
    let style = match line.level {
        Level::ERROR => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        Level::WARN => Style::new().fg(Color::Yellow),
        Level::INFO => Style::new().fg(Color::Gray),
        Level::DEBUG | Level::TRACE => Style::new().add_modifier(Modifier::DIM),
    };
    Line::styled(line.text.clone(), style)
}

pub fn draw_log_tail(frame: &mut Frame, area: Rect, logs: &[LogLine]) {
    let block = Block::bordered().title(" logs ").border_style(Color::Gray);
    let shown = block.inner(area).height as usize;
    let lines: Vec<Line> = logs[logs.len().saturating_sub(shown)..]
        .iter()
        .map(log_line)
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, state: &AgentState) {
    let [tabs, identity] = Layout::vertical([Constraint::Length(1); 2]).areas(area);
    let tab = |key: &'static str, name: &'static str, active: bool| {
        let style = if active {
            Style::new().fg(Color::Black).bg(Color::Cyan).bold()
        } else {
            Style::new().fg(Color::Cyan)
        };
        [
            Span::styled(format!(" {key} {name} "), style),
            Span::raw(" "),
        ]
    };
    let mut spans = vec![
        Span::styled(
            " imaged ",
            Style::new().fg(Color::Black).bg(Color::Yellow).bold(),
        ),
        Span::raw("  "),
    ];
    spans.extend(tab("F1", "Progress", app.tab == Tab::Progress));
    spans.extend(tab("F2", "Logs", app.tab == Tab::Logs));
    if app.shell {
        spans.extend(tab("F3", "Shell", false));
    }
    spans.extend(tab("F10", "Reboot", app.confirming_reboot));
    let connection = match &state.connection {
        Connection::Connecting => Span::styled("■ connecting", Color::Yellow),
        Connection::Connected => Span::styled("■ connected", Color::Green),
        Connection::Lost(reason) => Span::styled(format!("■ disconnected: {reason}"), Color::Red),
    };
    let [left, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(connection.width() as u16 + 1),
    ])
    .areas(tabs);
    frame.render_widget(Line::from(spans), left);
    frame.render_widget(Line::from(connection), right);

    let id = &state.identity;
    let ip = id.ip.as_deref().unwrap_or("no ip");
    frame.render_widget(
        Line::styled(
            format!(" mac {} · ip {ip} · server {}", id.mac, id.server),
            Style::new().fg(Color::Gray),
        ),
        identity,
    );
}

fn draw_idle(frame: &mut Frame, area: Rect, state: &AgentState, logs: &[LogLine]) {
    let [_, ip, waiting, _, tail] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(8),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(8),
    ])
    .areas(area);
    let ip_text = state.identity.ip.as_deref().unwrap_or("no ip");
    draw_big_or_plain(frame, ip, ip_text, Style::new().fg(Color::Cyan));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("waiting for a task").bold(),
            Line::styled(state.identity.mac.clone(), Color::Gray),
        ])
        .alignment(Alignment::Center),
        waiting,
    );
    draw_log_tail(frame, tail, logs);
}

fn draw_logs(frame: &mut Frame, area: Rect, logs: &[LogLine], scroll: usize) {
    let block = Block::bordered()
        .title(" logs · ↑↓ PgUp PgDn End ")
        .border_style(Color::Gray);
    let height = block.inner(area).height as usize;
    let end = logs
        .len()
        .saturating_sub(scroll.min(logs.len().saturating_sub(height)));
    let lines: Vec<Line> = logs[end.saturating_sub(height)..end]
        .iter()
        .map(log_line)
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_reboot_dialog(frame: &mut Frame, task_running: bool) {
    let [area] = Layout::horizontal([Constraint::Length(54)])
        .flex(Flex::Center)
        .areas(frame.area());
    let [area] = Layout::vertical([Constraint::Length(7)])
        .flex(Flex::Center)
        .areas(area);
    let mut lines = vec![Line::from("Reboot this machine now?").bold(), Line::raw("")];
    if task_running {
        lines.push(Line::styled(
            "The running task is interrupted and marked failed.",
            Style::new().fg(Color::Red).bold(),
        ));
    }
    lines.push(Line::from("y reboot · any other key cancels"));
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::bordered()
                .border_type(BorderType::Double)
                .border_style(Color::Yellow)
                .title(" reboot "),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_durations_read_at_a_glance() {
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(124.4e6), "124 MB");
        assert_eq!(format_bytes(60.5e9), "60.5 GB");
        assert_eq!(format_duration(Duration::from_secs(102)), "1:42");
        assert_eq!(format_duration(Duration::from_secs(3_725)), "1:02:05");
    }
}
