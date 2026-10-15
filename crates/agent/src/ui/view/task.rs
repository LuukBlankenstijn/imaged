use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize as _};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use tokio::time::Instant;

use super::multicast::{drops_health, loss_health, stall_health};
use super::{
    draw_big_or_plain, draw_log_tail, draw_task_title, format_bytes, format_duration,
    format_percent as percent,
};
use crate::logging::LogLine;
use crate::status::{Outcome, Phase, Reboot, ScriptPhase, Step, TaskState};

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    task: &TaskState,
    reboot: Reboot,
    logs: &[LogLine],
    now: Instant,
) {
    let [title, top, overall, steps, tail] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(10),
        Constraint::Length(1),
        Constraint::Length(steps_height(task)),
        Constraint::Fill(1),
    ])
    .areas(area);
    draw_task_title(frame, title, task, now);
    let fraction = task.fraction();
    if task.outcome.is_some() {
        draw_result_banner(frame, top, task, reboot, now);
    } else {
        let current = task
            .current_partition()
            .map(|(_, partition)| &partition.progress)
            .filter(|progress| progress.bytes_per_second > 0.0);
        let rate = current.map_or("-".to_string(), |progress| {
            format!("{}/s", format_bytes(progress.bytes_per_second))
        });
        let left = current
            .and_then(|progress| progress.remaining)
            .map_or("-".to_string(), format_duration);
        let [percent_box, rate_box, left_box] =
            Layout::horizontal([Constraint::Fill(1); 3]).areas(top);
        for (cell, title, value) in [
            (percent_box, " overall ", percent(fraction)),
            (rate_box, " rate ", rate),
            (left_box, " partition time left ", left),
        ] {
            let block = Block::bordered().title(title).border_style(Color::Gray);
            draw_big_or_plain(
                frame,
                block.inner(cell),
                &value,
                Style::new().fg(Color::Cyan),
            );
            frame.render_widget(block, cell);
        }
    }
    let mut overall_bar = Vec::from(bar(fraction, overall.width.saturating_sub(5)));
    overall_bar.push(Span::raw(format!(" {:>4}", percent(fraction))).bold());
    frame.render_widget(Line::from(overall_bar), overall);
    draw_steps(frame, steps, task);
    draw_log_tail(frame, tail, logs);
}

pub fn steps_height(task: &TaskState) -> u16 {
    3 + (task.partitions.len() + task.scripts.len()) as u16
}

pub fn draw_steps(frame: &mut Frame, area: Rect, task: &TaskState) {
    let block = Block::bordered().title(" steps ").border_style(Color::Gray);
    let bar_width = block.inner(area).width.saturating_sub(60);
    let failed = Style::new().fg(Color::Red).bold();
    let interrupted = task.outcome.is_some();
    let marker = |phase| match phase {
        Phase::Done => Span::styled("√", Color::Green),
        Phase::Running if interrupted => Span::styled("x", failed),
        Phase::Running => Span::styled("►", Style::new().fg(Color::Cyan).bold()),
        Phase::Waiting => Span::styled("·", Color::Gray),
    };
    let table = match task.step {
        Step::Starting => Phase::Waiting,
        Step::PartitionTable => Phase::Running,
        Step::Partitions | Step::Finishing => Phase::Done,
    };
    let mut lines = vec![Line::from(vec![
        marker(table),
        Span::raw(" partition table"),
    ])];
    lines.extend(task.partitions.iter().map(|partition| {
        let fraction = partition.fraction();
        let mut spans = vec![
            marker(partition.phase),
            Span::raw(format!(
                " partition {:<3} {:<4} {:>7}  ",
                partition.number,
                partition.filesystem.as_str(),
                format_bytes(partition.size as f64)
            )),
        ];
        spans.extend(bar(fraction, bar_width));
        spans.push(Span::raw(format!(" {:>4}", percent(fraction))));
        if partition.phase == Phase::Running
            && !interrupted
            && partition.progress.bytes_per_second > 0.0
        {
            spans.push(Span::raw(format!(
                "  {:>9}  {:>7} left",
                format!("{}/s", format_bytes(partition.progress.bytes_per_second)),
                partition
                    .progress
                    .remaining
                    .map_or("-".to_string(), format_duration)
            )));
        }
        Line::from(spans)
    }));
    let name_width = task
        .scripts
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    lines.extend(task.scripts.iter().map(|(name, phase)| {
        let (marker, status) = match phase {
            ScriptPhase::Waiting => (marker(Phase::Waiting), Span::raw("")),
            ScriptPhase::Running if interrupted => {
                (marker(Phase::Running), Span::styled("interrupted", failed))
            }
            ScriptPhase::Running => (marker(Phase::Running), Span::styled("running", Color::Cyan)),
            ScriptPhase::Exited(Some(0)) => (marker(Phase::Done), Span::styled("ok", Color::Green)),
            ScriptPhase::Exited(Some(code)) => (
                Span::styled("x", failed),
                Span::styled(format!("exit {code}"), failed),
            ),
            ScriptPhase::Exited(None) => {
                (Span::styled("x", failed), Span::styled("killed", failed))
            }
        };
        Line::from(vec![
            marker,
            Span::raw(format!(" script {name:<name_width$}  ")),
            status,
        ])
    }));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn draw_result_banner(
    frame: &mut Frame,
    area: Rect,
    task: &TaskState,
    reboot: Reboot,
    now: Instant,
) {
    let Some((outcome, took)) = &task.outcome else {
        return;
    };
    let (word, color) = match outcome {
        Outcome::Done => ("DONE", Color::Green),
        Outcome::Failed(_) => ("FAILED", Color::Red),
        Outcome::Cancelled => ("CANCELLED", Color::Yellow),
    };
    let mut lines = Vec::new();
    match outcome {
        Outcome::Done => {
            let count = |n: usize, noun: &str| match n {
                1 => format!("1 {noun}"),
                n => format!("{n} {noun}s"),
            };
            let mut summary = vec![format!("in {}", format_duration(*took))];
            if !task.partitions.is_empty() {
                summary.push(count(task.partitions.len(), "partition"));
            }
            if !task.scripts.is_empty() {
                summary.push(count(task.scripts.len(), "script"));
            }
            lines.push(Line::from(summary.join(" · ")));
        }
        Outcome::Failed(error) => lines.push(Line::styled(
            error.clone(),
            Style::new().fg(Color::Red).bold(),
        )),
        Outcome::Cancelled => {}
    }
    let reboot_text = match (reboot, outcome) {
        (Reboot::Countdown { until }, _) => Some(format!(
            "Rebooting in {:.0}... any key cancels",
            until.saturating_duration_since(now).as_secs_f64().ceil()
        )),
        (Reboot::Cancelled, _) => Some("Reboot cancelled · F10 to reboot".to_string()),
        (Reboot::NotPlanned, Outcome::Done) => None,
        (Reboot::NotPlanned, _) => Some("F10 to reboot".to_string()),
    };
    lines.extend(reboot_text.map(|text| Line::from(text).bold()));
    if let Some(multicast) = &task.multicast {
        lines.push(Line::from(vec![
            Span::raw(format!("naks {}", multicast.total_naks)),
            Span::raw(" · worst loss "),
            Span::styled(
                format!("{:.1}%", multicast.worst_loss * 100.0),
                loss_health(multicast.worst_loss).color(),
            ),
            Span::raw(" · worst stall "),
            Span::styled(
                format!("{} ms", multicast.worst_stall_ms),
                stall_health(multicast.worst_stall_ms).color(),
            ),
            Span::raw(" · overflow drops "),
            Span::styled(
                multicast.total_overflow_drops.to_string(),
                drops_health(multicast.total_overflow_drops).color(),
            ),
        ]));
    }
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(color);
    let [big, text] =
        Layout::vertical([Constraint::Length(4), Constraint::Fill(1)]).areas(block.inner(area));
    frame.render_widget(block, area);
    draw_big_or_plain(frame, big, word, Style::new().fg(color).bold());
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        text,
    );
}

fn bar(fraction: f64, width: u16) -> [Span<'static>; 2] {
    let filled = (fraction.clamp(0.0, 1.0) * f64::from(width)).round() as usize;
    [
        Span::styled("█".repeat(filled), Color::Cyan),
        Span::styled("░".repeat(usize::from(width) - filled), Color::Gray),
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use imaged_disk::{Filesystem, Progress};
    use imaged_model::TaskType;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use tracing::Level;

    use super::*;
    use crate::status::{AgentState, Connection, Identity, MulticastState, PartitionState};
    use crate::ui::{App, Tab};

    fn render(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(draw).unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    fn screen(buffer: &Buffer) -> String {
        rows(buffer).join("\n")
    }

    fn color_of(buffer: &Buffer, text: &str) -> Color {
        let (y, row) = rows(buffer)
            .into_iter()
            .enumerate()
            .find(|(_, row)| row.contains(text))
            .unwrap_or_else(|| panic!("{text:?} is not on screen"));
        let x = row[..row.find(text).unwrap()].chars().count();
        buffer[(x as u16, y as u16)].fg
    }

    fn partition(number: i64, filesystem: Filesystem, size: u64, phase: Phase) -> PartitionState {
        PartitionState {
            number,
            filesystem,
            size,
            phase,
            progress: Progress::default(),
        }
    }

    fn deploy(now: Instant) -> TaskState {
        let mut imaging = partition(2, Filesystem::Ext4, 3_000_000_000, Phase::Running);
        imaging.progress = Progress {
            fraction: 0.5,
            bytes_per_second: 40e6,
            remaining: Some(Duration::from_secs(75)),
        };
        TaskState {
            id: 7,
            kind: TaskType::Deploy,
            image_id: Some(3),
            started: now,
            step: Step::Partitions,
            partitions: vec![
                partition(1, Filesystem::Vfat, 1_000_000_000, Phase::Done),
                imaging,
                partition(3, Filesystem::Swap, 2_000_000_000, Phase::Waiting),
            ],
            scripts: vec![
                ("set-hostname".to_string(), ScriptPhase::Waiting),
                ("join-domain".to_string(), ScriptPhase::Waiting),
            ],
            multicast: None,
            outcome: None,
        }
    }

    fn ended(now: Instant, outcome: Outcome) -> TaskState {
        let mut task = deploy(now);
        task.step = Step::Finishing;
        for partition in &mut task.partitions {
            partition.phase = Phase::Done;
        }
        task.scripts[0].1 = ScriptPhase::Exited(Some(0));
        task.scripts[1].1 = ScriptPhase::Exited(Some(3));
        task.outcome = Some((outcome, Duration::from_secs(252)));
        task
    }

    fn logs() -> Vec<LogLine> {
        vec![LogLine {
            level: Level::INFO,
            text: "restoring partition 2".to_string(),
        }]
    }

    fn render_task(
        width: u16,
        height: u16,
        task: &TaskState,
        reboot: Reboot,
        now: Instant,
    ) -> Buffer {
        render(width, height, |frame| {
            draw(frame, frame.area(), task, reboot, &logs(), now)
        })
    }

    #[test]
    fn a_running_deploy_shows_big_numbers_partitions_and_scripts() {
        let now = Instant::now();
        let buffer = render_task(160, 50, &deploy(now), Reboot::NotPlanned, now);
        let rows = rows(&buffer);
        let overall_box: String = rows[1..11]
            .iter()
            .map(|row| row.chars().take(53).collect::<String>())
            .collect();

        assert!(overall_box.contains('█'));
        assert!(!overall_box.contains("62%"));
        assert!(rows[11].contains('█') && rows[11].ends_with("62%"));
        let imaging = rows.iter().find(|row| row.contains("partition 2")).unwrap();
        for text in ["►", "ext4", "3.0 GB", "50%", "40.0 MB/s", "1:15 left"] {
            assert!(imaging.contains(text), "{text:?} missing in {imaging:?}");
        }
        let screen = screen(&buffer);
        for text in [
            "√ partition table",
            "√ partition 1",
            "· partition 3",
            "· script set-hostname",
            "· script join-domain",
            "restoring partition 2",
        ] {
            assert!(screen.contains(text), "{text:?} missing\n{screen}");
        }
    }

    #[test]
    fn before_any_partition_runs_rate_and_time_left_show_a_dash() {
        let now = Instant::now();
        let mut task = deploy(now);
        task.step = Step::PartitionTable;
        task.partitions.clear();

        let screen = screen(&render_task(160, 50, &task, Reboot::NotPlanned, now));

        assert!(screen.contains("► partition table"));
        assert!(!screen.contains("MB/s"));
    }

    #[test]
    fn a_finished_deploy_counts_down_to_the_reboot_in_whole_seconds() {
        let now = Instant::now();
        let task = ended(now, Outcome::Done);
        let reboot = Reboot::Countdown {
            until: now + Duration::from_millis(3_400),
        };

        let buffer = render_task(160, 50, &task, reboot, now);
        let screen = screen(&buffer);

        assert!(screen.contains("in 4:12 · 3 partitions · 2 scripts"));
        assert!(screen.contains("Rebooting in 4... any key cancels"));
        assert_eq!(buffer[(0, 1)].symbol(), "╔");
        assert_eq!(buffer[(0, 1)].fg, Color::Green);
        assert_eq!(color_of(&buffer, "exit 3"), Color::Red);
        assert!(screen.contains("√ partition 3"));
    }

    #[test]
    fn a_cancelled_countdown_points_at_the_reboot_key() {
        let now = Instant::now();

        let screen = screen(&render_task(
            160,
            50,
            &ended(now, Outcome::Done),
            Reboot::Cancelled,
            now,
        ));

        assert!(screen.contains("Reboot cancelled · F10 to reboot"));
        assert!(!screen.contains("Rebooting in"));
    }

    #[test]
    fn a_failed_task_shows_its_error_in_red() {
        let now = Instant::now();
        let task = ended(now, Outcome::Failed("partclone exited with 1".to_string()));

        let buffer = render_task(160, 50, &task, Reboot::NotPlanned, now);
        let screen = screen(&buffer);

        assert!(screen.contains("partclone exited with 1"));
        assert!(screen.contains("F10 to reboot"));
        assert_eq!(color_of(&buffer, "partclone exited"), Color::Red);
        assert_eq!(buffer[(0, 1)].fg, Color::Red);
    }

    #[test]
    fn a_partition_cut_short_by_a_failure_is_crossed_out_without_its_rate() {
        let now = Instant::now();
        let mut task = deploy(now);
        task.outcome = Some((Outcome::Failed("disk vanished".to_string()), Duration::ZERO));

        let buffer = render_task(160, 50, &task, Reboot::NotPlanned, now);
        let screen = screen(&buffer);

        assert!(!screen.contains("►"));
        assert!(!screen.contains("40.0 MB/s"));
        assert_eq!(color_of(&buffer, "x partition 2"), Color::Red);
    }

    #[test]
    fn a_cancelled_task_is_marked_in_yellow() {
        let now = Instant::now();

        let buffer = render_task(
            160,
            50,
            &ended(now, Outcome::Cancelled),
            Reboot::NotPlanned,
            now,
        );

        assert_eq!(buffer[(0, 1)].fg, Color::Yellow);
        assert!(screen(&buffer).contains("F10 to reboot"));
    }

    #[test]
    fn a_finished_capture_does_not_mention_rebooting() {
        let now = Instant::now();
        let mut task = ended(now, Outcome::Done);
        task.kind = TaskType::Capture;
        task.scripts.clear();

        let screen = screen(&render_task(160, 50, &task, Reboot::NotPlanned, now));

        assert!(screen.contains("in 4:12 · 3 partitions"));
        assert!(!screen.contains("scripts"));
        assert!(!screen.contains("eboot"));
    }

    #[test]
    fn a_finished_multicast_shows_its_totals_coloured_by_health() {
        let now = Instant::now();
        let mut task = ended(now, Outcome::Done);
        task.multicast = Some(MulticastState {
            total_naks: 35,
            worst_loss: 0.02,
            worst_stall_ms: 300,
            total_overflow_drops: 17,
            ..MulticastState::default()
        });

        let buffer = render(160, 12, |frame| {
            draw_result_banner(frame, frame.area(), &task, Reboot::NotPlanned, now)
        });

        assert!(
            screen(&buffer)
                .contains("naks 35 · worst loss 2.0% · worst stall 300 ms · overflow drops 17")
        );
        assert_eq!(color_of(&buffer, "2.0%"), loss_health(0.02).color());
        assert_eq!(color_of(&buffer, "300 ms"), stall_health(300).color());
        assert_eq!(color_of(&buffer, "17"), drops_health(17).color());
    }

    #[test]
    fn a_vga_text_console_still_shows_the_progress() {
        let now = Instant::now();
        let state = AgentState {
            identity: Identity {
                mac: "52:54:00:12:34:56".to_string(),
                ip: Some("10.0.0.7".to_string()),
                server: "http://10.0.0.1:8080".to_string(),
            },
            connection: Connection::Connected,
            task: Some(deploy(now)),
            reboot: Reboot::NotPlanned,
        };
        let app = App {
            tab: Tab::Progress,
            confirming_reboot: false,
            log_scroll: 0,
            shell: true,
        };

        let buffer = render(80, 25, |frame| {
            super::super::draw(frame, &app, &state, &logs(), now)
        });
        let screen = screen(&buffer);

        assert!(screen.contains("62%"));
        assert!(screen.contains("partition 2"));
    }

    #[test]
    fn tiny_terminals_do_not_crash_the_screen() {
        let now = Instant::now();
        for (width, height) in [(1, 1), (10, 3), (40, 8), (80, 12)] {
            render_task(width, height, &deploy(now), Reboot::NotPlanned, now);
            render_task(
                width,
                height,
                &ended(now, Outcome::Failed("disk vanished".to_string())),
                Reboot::Countdown { until: now },
                now,
            );
        }
    }
}
