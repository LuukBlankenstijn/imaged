use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, RenderDirection, Sparkline};
use scuttlecast::state::ReceiveLimit;
use tokio::time::Instant;

use super::{
    Health, draw_big_or_plain, draw_log_tail, draw_task_title, format_bytes, format_duration,
    format_percent, task,
};
use crate::logging::LogLine;
use crate::status::{MulticastState, Outcome, Reboot, TaskState};

const LOSS_WARNING: f64 = 0.01;
const LOSS_BAD: f64 = 0.05;
const STALL_WARNING_MS: u32 = 50;
const STALL_BAD_MS: u32 = 250;
const NAKS_PER_SECOND_WARNING: f64 = 10.0;
const NAKS_PER_SECOND_BAD: f64 = 100.0;
const DROPS_BAD: u64 = 1;

const RESULT_BANNER_ROWS: u16 = 9;
const MIN_LOG_ROWS: u16 = 5;
const VALUE_AND_CHART_ROWS: [(u16, u16); 2] = [(8, 3), (4, 1)];

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    task: &TaskState,
    multicast: &MulticastState,
    reboot: Reboot,
    logs: &[LogLine],
    now: Instant,
) {
    let receive = &multicast.receive;
    let banner_rows = if task.outcome.is_some() {
        RESULT_BANNER_ROWS
    } else {
        0
    };
    let steps_rows = task::steps_height(task);
    let room = area
        .height
        .saturating_sub(1 + banner_rows + steps_rows + MIN_LOG_ROWS);
    let (value_rows, chart_rows) = VALUE_AND_CHART_ROWS
        .into_iter()
        .find(|&(value, chart)| 2 * (value + 3) + chart <= room)
        .unwrap_or((1, 1));
    let [title, banner, top, bottom, steps, tail] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(banner_rows),
        Constraint::Length(value_rows + chart_rows + 3),
        Constraint::Length(value_rows + 3),
        Constraint::Max(steps_rows),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [status, network, disk] = Layout::horizontal([Constraint::Fill(1); 3]).areas(top);
    let [progress, loss, naks, drops] = Layout::horizontal([
        Constraint::Fill(2),
        Constraint::Fill(1),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ])
    .areas(bottom);

    draw_task_title(frame, title, task, now);
    if task.outcome.is_some() {
        task::draw_result_banner(frame, banner, task, reboot, now);
    }

    let (status_health, status_value, status_detail) = match receive.limiting {
        _ if matches!(task.outcome, Some((Outcome::Done, _))) => {
            (Health::Good, "DONE", "every file received".to_string())
        }
        _ if task.outcome.is_some() => (
            Health::Bad,
            "STOPPED",
            "the task did not finish".to_string(),
        ),
        _ if receive.transfer_id == 0 => (
            Health::Idle,
            "JOINING",
            "waiting for the sender".to_string(),
        ),
        ReceiveLimit::KeepingUp => (Health::Good, "OK", "keeping up with the sender".to_string()),
        ReceiveLimit::SenderIdle { silent_ms } => (
            Health::Idle,
            "WAITING",
            format!("sender quiet {:.1} s", f64::from(silent_ms) / 1000.0),
        ),
        ReceiveLimit::RepairingLoss { loss, naks } => (
            Health::Warning,
            "REPAIRING",
            format!("loss {:.1}% · {naks} naks", loss * 100.0),
        ),
        ReceiveLimit::WriteStalled { stall_ms, .. } => (
            Health::Bad,
            "DISK SLOW",
            format!("writes stalled {stall_ms} ms"),
        ),
        ReceiveLimit::BufferOverflowing { dropped } => (
            Health::Bad,
            "OVERFLOW",
            format!("kernel dropped {dropped} datagrams"),
        ),
        ReceiveLimit::RepairStalled { missing, .. } => (
            Health::Bad,
            "STALLED",
            format!("waiting for {missing} missing shards"),
        ),
    };
    draw_panel(
        frame,
        status,
        "STATUS",
        status_health,
        status_value,
        &status_detail,
        value_rows,
    );

    let rate = receive.bytes_per_second;
    let network_health = if rate > 0.0 {
        Health::Good
    } else {
        Health::Idle
    };
    let peak = multicast.rates.iter().copied().fold(0.0, f64::max);
    let chart = draw_panel(
        frame,
        network,
        "NETWORK",
        network_health,
        &format!("{}/s", format_bytes(rate)),
        &format!("peak {}/s", format_bytes(peak)),
        value_rows,
    );
    frame.render_widget(
        Sparkline::default()
            .data(multicast.rates.iter().rev().map(|&rate| rate as u64))
            .direction(RenderDirection::RightToLeft)
            .bar_set(symbols::bar::THREE_LEVELS)
            .style(network_health.color()),
        chart,
    );

    let disk_rate = task
        .current_partition()
        .map(|(_, partition)| partition.progress.bytes_per_second)
        .filter(|&rate| rate > 0.0)
        .map_or_else(
            || "-".to_string(),
            |rate| format!("{}/s", format_bytes(rate)),
        );
    draw_panel(
        frame,
        disk,
        "DISK",
        match receive.limiting {
            ReceiveLimit::WriteStalled { .. } => Health::Bad,
            _ => stall_health(receive.sink_stall_ms),
        },
        &disk_rate,
        &format!(
            "stall {} · worst {} ms",
            receive.sink_stall_ms, multicast.worst_stall_ms
        ),
        value_rows,
    );

    let file = match multicast.slot {
        0 => "partition table".to_string(),
        number => format!("partition {number}"),
    };
    let received = receive.fraction_complete().map_or_else(
        || format_bytes(receive.received_bytes as f64),
        |fraction| format!("{:.0}%", fraction * 100.0),
    );
    let eta = receive
        .announced_bytes
        .filter(|_| rate > 0.0)
        .and_then(|announced| {
            Duration::try_from_secs_f64(
                announced.saturating_sub(receive.received_bytes) as f64 / rate,
            )
            .ok()
        })
        .map_or_else(String::new, |left| {
            format!(" · eta {}", format_duration(left))
        });
    let fraction = task.fraction();
    draw_panel(
        frame,
        progress,
        "PROGRESS",
        if fraction > 0.0 {
            Health::Good
        } else {
            Health::Idle
        },
        &format_percent(fraction),
        &format!("{file} · {received}{eta}"),
        value_rows,
    );

    draw_panel(
        frame,
        loss,
        "LOSS",
        loss_health(receive.windowed_loss),
        &format!("{:.1}%", receive.windowed_loss * 100.0),
        &format!("worst {:.1}%", multicast.worst_loss * 100.0),
        value_rows,
    );
    draw_panel(
        frame,
        naks,
        "NAKS",
        grade(
            multicast.naks_per_second,
            NAKS_PER_SECOND_WARNING,
            NAKS_PER_SECOND_BAD,
        ),
        &format!("{:.0}/s", multicast.naks_per_second),
        &format!("{} total", multicast.total_naks),
        value_rows,
    );
    draw_panel(
        frame,
        drops,
        "DROPS",
        drops_health(multicast.total_overflow_drops),
        &multicast.total_overflow_drops.to_string(),
        &format!("{} this file", receive.overflow_drops),
        value_rows,
    );

    task::draw_steps(frame, steps, task);
    draw_log_tail(frame, tail, logs);
}

pub fn loss_health(loss: f64) -> Health {
    grade(loss, LOSS_WARNING, LOSS_BAD)
}

pub fn stall_health(stall_ms: u32) -> Health {
    grade(stall_ms, STALL_WARNING_MS, STALL_BAD_MS)
}

pub fn drops_health(drops: u64) -> Health {
    if drops < DROPS_BAD {
        Health::Good
    } else {
        Health::Bad
    }
}

fn grade<T: PartialOrd>(value: T, warning: T, bad: T) -> Health {
    if value < warning {
        Health::Good
    } else if value < bad {
        Health::Warning
    } else {
        Health::Bad
    }
}

fn draw_panel(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    health: Health,
    value: &str,
    detail: &str,
    value_rows: u16,
) -> Rect {
    let style = Style::new().fg(health.color());
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(style)
        .title(format!(" {title} "))
        .title_style(style.bold());
    let [value_area, free, detail_area] = Layout::vertical([
        Constraint::Length(value_rows),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(block.inner(area));
    frame.render_widget(block, area);
    draw_big_or_plain(frame, value_area, value, style);
    frame.render_widget(Line::styled(detail, Color::Gray).centered(), detail_area);
    free
}

#[cfg(test)]
mod tests {
    use imaged_disk::{Filesystem, Progress};
    use imaged_model::TaskType;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use scuttlecast::state::ReceiveState;

    use super::*;
    use crate::status::{Outcome, PartitionState, Phase, Step};

    const TITLES: [&str; 7] = [
        "STATUS", "NETWORK", "DISK", "PROGRESS", "LOSS", "NAKS", "DROPS",
    ];

    fn imaging() -> TaskState {
        TaskState {
            id: 12,
            kind: TaskType::Multicast,
            image_id: Some(3),
            started: Instant::now(),
            step: Step::Partitions,
            partitions: vec![PartitionState {
                number: 2,
                filesystem: Filesystem::Ext4,
                size: 4_000_000_000,
                phase: Phase::Running,
                progress: Progress {
                    fraction: 0.5,
                    bytes_per_second: 80e6,
                    remaining: Some(Duration::from_secs(25)),
                },
            }],
            scripts: Vec::new(),
            multicast: None,
            outcome: None,
        }
    }

    fn healthy() -> ReceiveState {
        ReceiveState {
            transfer_id: 7,
            announced_bytes: Some(4_000_000_000),
            received_bytes: 2_000_000_000,
            bytes_per_second: 118e6,
            ..ReceiveState::default()
        }
    }

    fn receiving(receive: ReceiveState) -> MulticastState {
        MulticastState {
            slot: 2,
            receive,
            rates: [90e6, 110e6, 0.0, 118e6].into(),
            ..MulticastState::default()
        }
    }

    fn render(width: u16, height: u16, task: &TaskState, multicast: &MulticastState) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    task,
                    multicast,
                    Reboot::NotPlanned,
                    &[],
                    Instant::now(),
                )
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buffer: &Buffer) -> String {
        buffer
            .content
            .chunks(buffer.area.width.max(1) as usize)
            .map(|row| row.iter().map(Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn panel_corner(buffer: &Buffer, title: &str) -> (u16, u16) {
        let heading = format!("╔ {title} ");
        text(buffer)
            .lines()
            .enumerate()
            .find_map(|(y, row)| {
                let x = row[..row.find(&heading)?].chars().count();
                Some((x as u16, y as u16))
            })
            .unwrap_or_else(|| panic!("no {title} panel in\n{}", text(buffer)))
    }

    fn panel_colors(buffer: &Buffer, title: &str) -> (Color, Color) {
        let (x, y) = panel_corner(buffer, title);
        (buffer[(x, y)].fg, buffer[(x + 2, y)].fg)
    }

    #[test]
    fn a_healthy_transfer_is_green_across_the_board() {
        let buffer = render(160, 48, &imaging(), &receiving(healthy()));

        for title in TITLES {
            assert_eq!(
                panel_colors(&buffer, title),
                (Color::Green, Color::Green),
                "{title}"
            );
        }
        assert!(text(&buffer).contains("keeping up with the sender"));
    }

    #[test]
    fn a_disk_that_cannot_keep_up_turns_status_and_disk_red() {
        let multicast = receiving(ReceiveState {
            sink_stall_ms: 400,
            limiting: ReceiveLimit::WriteStalled {
                stall_ms: 400,
                held_at: 9,
            },
            ..healthy()
        });

        let buffer = render(160, 48, &imaging(), &multicast);
        assert_eq!(panel_colors(&buffer, "STATUS"), (Color::Red, Color::Red));
        assert_eq!(panel_colors(&buffer, "DISK"), (Color::Red, Color::Red));
        let screen = text(&render(80, 25, &imaging(), &multicast));
        assert!(screen.contains("DISK SLOW"));
        assert!(screen.contains("writes stalled 400 ms"));
    }

    #[test]
    fn the_disk_is_red_whenever_scuttlecast_blames_it_even_for_a_short_stall() {
        let multicast = receiving(ReceiveState {
            sink_stall_ms: 73,
            limiting: ReceiveLimit::WriteStalled {
                stall_ms: 73,
                held_at: 9,
            },
            ..healthy()
        });

        let buffer = render(160, 48, &imaging(), &multicast);

        assert_eq!(panel_colors(&buffer, "DISK"), (Color::Red, Color::Red));
    }

    #[test]
    fn loss_between_one_and_five_percent_is_a_warning() {
        let multicast = receiving(ReceiveState {
            windowed_loss: 0.02,
            limiting: ReceiveLimit::RepairingLoss {
                loss: 0.02,
                naks: 34,
            },
            ..healthy()
        });

        let buffer = render(160, 48, &imaging(), &multicast);
        assert_eq!(
            panel_colors(&buffer, "LOSS"),
            (Color::Yellow, Color::Yellow)
        );
        assert_eq!(
            panel_colors(&buffer, "STATUS"),
            (Color::Yellow, Color::Yellow)
        );
        assert!(text(&buffer).contains("loss 2.0% · 34 naks"));
    }

    #[test]
    fn a_receiver_that_joined_before_the_sender_started_waits_in_blue() {
        let multicast = MulticastState {
            slot: 2,
            ..MulticastState::default()
        };

        let buffer = render(160, 48, &imaging(), &multicast);
        assert_eq!(panel_colors(&buffer, "STATUS"), (Color::Blue, Color::Blue));
        assert_eq!(panel_colors(&buffer, "NETWORK"), (Color::Blue, Color::Blue));
        let screen = text(&render(80, 25, &imaging(), &multicast));
        assert!(screen.contains("JOINING"));
        assert!(screen.contains("waiting for the sender"));
    }

    #[test]
    fn the_progress_panel_names_the_file_and_the_time_it_still_needs() {
        let screen = text(&render(80, 25, &imaging(), &receiving(healthy())));

        assert!(screen.contains("partition 2 · 50% · eta 0:16"));
    }

    #[test]
    fn health_turns_at_the_documented_thresholds() {
        assert_eq!(loss_health(0.0099), Health::Good);
        assert_eq!(loss_health(0.01), Health::Warning);
        assert_eq!(loss_health(0.0499), Health::Warning);
        assert_eq!(loss_health(0.05), Health::Bad);
        assert_eq!(stall_health(49), Health::Good);
        assert_eq!(stall_health(50), Health::Warning);
        assert_eq!(stall_health(249), Health::Warning);
        assert_eq!(stall_health(250), Health::Bad);
        assert_eq!(drops_health(0), Health::Good);
        assert_eq!(drops_health(1), Health::Bad);
    }

    #[test]
    fn the_vga_text_console_shows_all_seven_panels() {
        let buffer = render(80, 25, &imaging(), &receiving(healthy()));

        for title in TITLES {
            panel_corner(&buffer, title);
        }
    }

    #[test]
    fn a_large_console_draws_the_status_in_block_digits() {
        let buffer = render(240, 67, &imaging(), &receiving(healthy()));

        let (left, top) = panel_corner(&buffer, "STATUS");
        let digits = (top + 1..top + 9)
            .flat_map(|y| (left..left + 80).map(move |x| (x, y)))
            .filter(|&position| buffer[position].symbol() == "█")
            .count();
        assert!(digits > 20, "{}", text(&buffer));
        assert!(!text(&buffer).contains(" OK "));
    }

    #[test]
    fn a_finished_run_keeps_its_final_stats_under_the_result_banner() {
        let task = TaskState {
            outcome: Some((Outcome::Done, Duration::from_secs(600))),
            ..imaging()
        };
        let multicast = MulticastState {
            total_overflow_drops: 12,
            ..receiving(healthy())
        };

        let buffer = render(160, 48, &task, &multicast);
        for title in TITLES {
            panel_corner(&buffer, title);
        }
        assert_eq!(panel_colors(&buffer, "DROPS"), (Color::Red, Color::Red));
        render(80, 25, &task, &multicast);
    }

    #[test]
    fn a_finished_run_reports_done_instead_of_waiting_for_another_file() {
        let task = TaskState {
            outcome: Some((Outcome::Done, Duration::from_secs(600))),
            ..imaging()
        };
        let idle_after_the_last_file = receiving(ReceiveState::default());

        let buffer = render(160, 48, &task, &idle_after_the_last_file);

        assert_eq!(
            panel_colors(&buffer, "STATUS"),
            (Color::Green, Color::Green)
        );
        assert!(text(&buffer).contains("every file received"));
        assert!(!text(&buffer).contains("JOINING"));
    }

    #[test]
    fn empty_history_unknown_sizes_and_tiny_screens_do_not_panic() {
        let starting = TaskState {
            step: Step::Starting,
            partitions: Vec::new(),
            ..imaging()
        };
        let finished = TaskState {
            outcome: Some((Outcome::Failed("disk vanished".to_string()), Duration::ZERO)),
            ..starting.clone()
        };
        let unknown_size = receiving(ReceiveState {
            announced_bytes: None,
            bytes_per_second: 0.0,
            ..healthy()
        });

        for (width, height) in [(1, 1), (20, 6), (80, 25), (240, 67)] {
            for task in [&starting, &finished] {
                render(width, height, task, &MulticastState::default());
                render(width, height, task, &unknown_size);
            }
        }
    }
}
