use dioxus::prelude::*;

use crate::api::tasks::{cancel_task, get_all_tasks, get_script_runs, kick_host, retry_task};
use crate::components::hooks::{use_poll, use_poll_while};
use crate::components::icons::Icon;
use crate::components::modal::ConfirmDialog;
use crate::components::progress::use_task_progress;
use crate::components::toast::toast_error;
use crate::components::ui::{
    Badge, Button, ButtonVariant, Card, EmptyState, PageHeader, Spinner, TaskStateBadge,
    TaskTypeBadge, Tone,
};
use crate::format::{format_bytes, format_duration, format_relative};
use crate::model::{
    HostProgress, KickRequest, MulticastProgress, ReceiverProgress, ScriptRun, Stage, Task,
    TaskHost, TaskProgress, TaskState, TaskType, TransferPhase,
};

fn select_class() -> &'static str {
    "rounded-md border border-ink-600 bg-ink-900 px-3 py-1.5 text-sm text-fog-200 focus:outline-none focus-visible:glow-amber"
}

#[component]
pub fn Tasks() -> Element {
    let mut tasks = use_poll(1500, || async move { get_all_tasks().await });
    let mut status = use_signal(|| "active".to_string());
    let mut kind = use_signal(|| "all".to_string());
    let mut host = use_signal(String::new);

    rsx! {
        PageHeader { title: "Tasks", subtitle: "Capture, deploy, multicast and reboot activity" }
        Card { class: "mb-4 p-3",
            div { class: "flex flex-wrap items-center gap-2",
                select {
                    class: select_class(),
                    value: "{status}",
                    onchange: move |e| status.set(e.value()),
                    option { value: "active", "Active" }
                    option { value: "all", "All" }
                    option { value: "done", "Done" }
                    option { value: "failed", "Failed" }
                    option { value: "cancelled", "Cancelled" }
                }
                select {
                    class: select_class(),
                    value: "{kind}",
                    onchange: move |e| kind.set(e.value()),
                    option { value: "all", "All types" }
                    option { value: "capture", "Capture" }
                    option { value: "deploy", "Deploy" }
                    option { value: "multicast", "Multicast" }
                    option { value: "reboot", "Reboot" }
                }
                input {
                    class: "{select_class()} w-40",
                    r#type: "text",
                    placeholder: "Host id",
                    value: "{host}",
                    oninput: move |e| host.set(e.value()),
                }
            }
        }
        {
            match &*tasks.read() {
                Some(Ok(list)) => {
                    let host_id = host().trim().parse::<i64>().ok();
                    let sf = status();
                    let kf = kind();
                    let mut items: Vec<Task> = list
                        .iter()
                        .filter(|t| status_matches(&sf, t.state))
                        .filter(|t| kind_matches(&kf, t.r#type))
                        .filter(|t| host_id.is_none_or(|id| t.hosts.iter().any(|h| h.host_id == id)))
                        .cloned()
                        .collect();
                    items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
                    if items.is_empty() {
                        rsx! {
                            Card { EmptyState { title: "No matching tasks", hint: "Adjust the filters above." } }
                        }
                    } else {
                        rsx! {
                            div { class: "flex flex-col gap-2",
                                for (i , t) in items.iter().enumerate() {
                                    TaskRow {
                                        key: "{t.id}",
                                        task: t.clone(),
                                        index: i,
                                        on_changed: move |_| tasks.restart(),
                                    }
                                }
                            }
                        }
                    }
                }
                Some(Err(e)) => rsx! {
                    Card { EmptyState { title: "Failed to load tasks", hint: e.to_string() } }
                },
                None => rsx! {
                    div { class: "flex justify-center py-16", Spinner {} }
                },
            }
        }
    }
}

fn status_matches(filter: &str, state: TaskState) -> bool {
    match filter {
        "active" => matches!(
            state,
            TaskState::Pending | TaskState::Running | TaskState::Partial
        ),
        "done" => state == TaskState::Done,
        "failed" => state == TaskState::Failed,
        "cancelled" => state == TaskState::Cancelled,
        _ => true,
    }
}

fn kind_matches(filter: &str, kind: TaskType) -> bool {
    match filter {
        "capture" => kind == TaskType::Capture,
        "deploy" => kind == TaskType::Deploy,
        "multicast" => kind == TaskType::Multicast,
        "reboot" => kind == TaskType::Reboot,
        _ => true,
    }
}

#[component]
fn TaskRow(task: Task, index: usize, on_changed: EventHandler<()>) -> Element {
    let task_id = task.id;
    let mut expanded = use_signal(|| false);
    let mut cancel_open = use_signal(|| false);
    let mut kick_target = use_signal(|| None::<i64>);
    let delay = index * 35;

    let can_cancel = matches!(task.state, TaskState::Pending | TaskState::Running);
    let can_retry = matches!(
        task.state,
        TaskState::Cancelled | TaskState::Failed | TaskState::Partial
    ) && !task.hosts.is_empty()
        && !(task.image_id.is_some() && task.image_deleted);
    let retryable_state = matches!(
        task.state,
        TaskState::Cancelled | TaskState::Failed | TaskState::Partial
    );
    let restores_disk = matches!(task.r#type, TaskType::Deploy | TaskType::Multicast);

    let image_label = task
        .image_name
        .clone()
        .unwrap_or_else(|| "\u{2014}".to_string());
    let host_count = task.hosts.len();
    let progress = use_task_progress(task_id);
    let multicast = progress.as_ref().and_then(|p| p.multicast.as_ref());
    let hosts: Vec<_> = task
        .hosts
        .iter()
        .map(|h| {
            let running = h.state == TaskState::Running;
            let reported = progress
                .as_ref()
                .filter(|_| running)
                .and_then(|p| p.hosts.get(&h.host_id).cloned());
            let receiver = multicast
                .filter(|_| running)
                .and_then(|m| m.receivers.iter().find(|r| r.host_id == h.host_id).cloned());
            (h.clone(), reported, receiver)
        })
        .collect();
    let receiving = multicast.is_some();

    let do_cancel = move || {
        spawn(async move {
            match cancel_task(task_id).await {
                Ok(_) => on_changed.call(()),
                Err(err) => toast_error("Cancel failed", err.to_string()),
            }
        });
    };

    let mut do_kick = move |host_id: i64| {
        kick_target.set(None);
        spawn(async move {
            match kick_host(KickRequest { task_id, host_id }).await {
                Ok(_) => on_changed.call(()),
                Err(err) => toast_error("Kick failed", err.to_string()),
            }
        });
    };

    rsx! {
        Card {
            class: "rise overflow-hidden",
            div {
                class: "flex cursor-pointer items-center gap-3 px-4 py-3 hover:bg-ink-800/40",
                style: "--d: {delay}ms",
                onclick: move |_| expanded.toggle(),
                Icon { name: "chevron", class: if expanded() { "w-4 h-4 rotate-180 transition-transform text-fog-500" } else { "w-4 h-4 transition-transform text-fog-500" } }
                TaskTypeBadge { r#type: task.r#type }
                TaskStateBadge { state: task.state }
                div { class: "min-w-0 flex-1 truncate text-sm text-fog-300",
                    span { class: "font-mono text-xs text-fog-500", "#{task.id} " }
                    span { "{image_label}" }
                    if task.image_deleted {
                        span { class: "ml-2 text-xs text-bad", "(image deleted)" }
                    }
                }
                span { class: "font-mono text-xs text-fog-500", "{host_count} host(s)" }
                span { class: "font-mono text-xs text-fog-500", "{format_relative(task.created_at)}" }
                div { class: "flex items-center gap-1.5", onclick: move |e: MouseEvent| e.stop_propagation(),
                    if can_cancel {
                        Button {
                            variant: ButtonVariant::Danger,
                            onclick: move |_| {
                                if restores_disk {
                                    cancel_open.set(true);
                                } else {
                                    do_cancel();
                                }
                            },
                            "Cancel"
                        }
                    }
                    if retryable_state {
                        Button {
                            variant: ButtonVariant::Ghost,
                            disabled: !can_retry,
                            onclick: move |_| {
                                spawn(async move {
                                    match retry_task(task_id).await {
                                        Ok(_) => on_changed.call(()),
                                        Err(err) => toast_error("Retry failed", err.to_string()),
                                    }
                                });
                            },
                            "Retry"
                        }
                    }
                }
            }
            if let Some(progress) = progress {
                TaskProgressBar { hosts: task.hosts.clone(), progress }
            }
            if expanded() {
                div { class: "border-t border-line/60 px-4 py-3",
                    div { class: "flex flex-col gap-2",
                        for (h, reported, receiver) in hosts.iter() {
                            TaskHostRow {
                                key: "{h.host_id}",
                                task_id,
                                host: h.clone(),
                                progress: reported.clone(),
                                receiver: receiver.clone(),
                                kickable: receiving && h.state == TaskState::Running,
                                on_kick: move |host_id| kick_target.set(Some(host_id)),
                                has_scripts: restores_disk,
                            }
                        }
                    }
                }
            }
            ConfirmDialog {
                open: cancel_open(),
                title: "Cancel task",
                message: "Cancelling interrupts an in-progress disk operation on the target hosts. Continue?",
                confirm_label: "Cancel task",
                danger: true,
                onconfirm: move |_| {
                    cancel_open.set(false);
                    do_cancel();
                },
                oncancel: move |_| cancel_open.set(false),
            }
            ConfirmDialog {
                open: kick_target().is_some(),
                title: "Kick host",
                message: "Kicking stops the multicast write on this host and marks it failed. Continue?",
                confirm_label: "Kick",
                danger: true,
                onconfirm: move |_| {
                    if let Some(host_id) = kick_target() {
                        do_kick(host_id);
                    }
                },
                oncancel: move |_| kick_target.set(None),
            }
        }
    }
}

#[component]
fn TaskHostRow(
    task_id: i64,
    host: TaskHost,
    progress: Option<HostProgress>,
    receiver: Option<ReceiverProgress>,
    kickable: bool,
    on_kick: EventHandler<i64>,
    has_scripts: bool,
) -> Element {
    let mut scripts_open = use_signal(|| false);
    let host_id = host.host_id;
    let running = matches!(host.state, TaskState::Pending | TaskState::Running);

    rsx! {
        div {
            div { class: "flex items-center gap-3 text-sm",
                div { class: "flex min-w-0 flex-1 flex-wrap items-center gap-3",
                    span { class: "font-mono text-xs text-fog-500", "host {host_id}" }
                    TaskStateBadge { state: host.state }
                    if let Some(started) = host.started_at {
                        span { class: "font-mono text-xs text-fog-600", "start {format_relative(started)}" }
                    }
                    if let Some(finished) = host.finished_at {
                        span { class: "font-mono text-xs text-fog-600", "end {format_relative(finished)}" }
                    }
                    if let Some(err) = host.error.clone() {
                        span { class: "text-xs text-bad", "{err}" }
                    }
                }
                div { class: "flex shrink-0 items-center gap-2",
                    if kickable {
                        Button {
                            variant: ButtonVariant::Danger,
                            class: "px-2 py-1 text-xs",
                            onclick: move |_| on_kick.call(host_id),
                            "Kick"
                        }
                    }
                    if has_scripts {
                        Button {
                            variant: if scripts_open() { ButtonVariant::Primary } else { ButtonVariant::Subtle },
                            class: "px-2 py-1 text-xs",
                            onclick: move |_| scripts_open.toggle(),
                            Icon { name: "script", class: "w-3.5 h-3.5" }
                            "Script output"
                        }
                    }
                }
            }
            if progress.is_some() || receiver.is_some() {
                div { class: "mt-1.5 ml-1 flex flex-col gap-1 border-l-2 border-line pl-3",
                    if let Some(progress) = progress {
                        HostProgressStats { progress }
                    }
                    if let Some(receiver) = receiver {
                        ReceiverStats { receiver }
                    }
                }
            }
            if scripts_open() {
                ScriptRuns { task_id, host_id, running }
            }
        }
    }
}

#[component]
fn ScriptRuns(task_id: i64, host_id: i64, running: ReadSignal<bool>) -> Element {
    let runs = use_poll_while(
        1500,
        move || *running.read(),
        move || async move { get_script_runs(task_id, host_id).await },
    );

    rsx! {
        div { class: "mt-2 mb-1 flex flex-col gap-2 border-l-2 border-line pl-3",
            match &*runs.read() {
                Some(Ok(list)) if list.is_empty() => rsx! {
                    p { class: "text-xs text-fog-600", "No post-install scripts have run on this host." }
                },
                Some(Ok(list)) => rsx! {
                    for run in list.iter() {
                        ScriptRunView { key: "{run.position}", run: run.clone() }
                    }
                },
                Some(Err(e)) => rsx! {
                    p { class: "text-xs text-bad", "Failed to load script output: {e}" }
                },
                None => rsx! {
                    Spinner { class: "w-4 h-4" }
                },
            }
        }
    }
}

#[component]
fn ScriptRunView(run: ScriptRun) -> Element {
    let (tone, status) = match (run.exit_code, run.finished_at) {
        (_, None) => (Tone::Run, "running".to_string()),
        (Some(0), Some(_)) => (Tone::Ok, "ok".to_string()),
        (Some(code), Some(_)) => (Tone::Bad, format!("exit {code}")),
        (None, Some(_)) => (Tone::Bad, "no exit code".to_string()),
    };
    let timing = match run.finished_at {
        Some(finished) => {
            let seconds = (finished - run.started_at).max(0) as u64 / 1000;
            format_duration(seconds)
        }
        None => format!("started {}", format_relative(run.started_at)),
    };

    rsx! {
        div { class: "overflow-hidden rounded-md border border-line bg-ink-900/60",
            div { class: "flex items-center gap-3 border-b border-line/60 px-3 py-1.5",
                span { class: "font-mono text-xs text-fog-600", "{run.position + 1}" }
                span { class: "min-w-0 flex-1 truncate font-mono text-sm text-fog-100", "{run.script_name}" }
                span { class: "font-mono text-[11px] text-fog-500", "{timing}" }
                Badge { tone, "{status}" }
            }
            if run.output.is_empty() {
                p { class: "px-3 py-2 font-mono text-xs text-fog-600", "no output" }
            } else {
                pre { class: "max-h-80 overflow-auto whitespace-pre-wrap break-all px-3 py-2 font-mono text-xs leading-relaxed text-fog-300",
                    "{run.output}"
                }
            }
        }
    }
}

#[component]
fn ProgressBar(fraction: f64) -> Element {
    let percent = (fraction * 100.0).clamp(0.0, 100.0);
    rsx! {
        div { class: "flex flex-1 items-center gap-3",
            div { class: "h-1.5 flex-1 overflow-hidden rounded-full bg-ink-700",
                div {
                    class: "h-full rounded-full bg-run transition-all duration-500",
                    style: "width: {percent}%",
                }
            }
            span { class: "w-10 text-right font-mono text-xs text-fog-300", "{percent:.0}%" }
        }
    }
}

fn host_fraction(host: &TaskHost, progress: &TaskProgress) -> Option<f64> {
    match host.state {
        TaskState::Pending => Some(0.0),
        TaskState::Running => Some(
            progress
                .hosts
                .get(&host.host_id)
                .map_or(0.0, |p| p.fraction),
        ),
        TaskState::Done => Some(1.0),
        TaskState::Failed | TaskState::Cancelled | TaskState::Partial => None,
    }
}

#[component]
fn TaskProgressBar(hosts: Vec<TaskHost>, progress: TaskProgress) -> Element {
    let fractions: Vec<f64> = hosts
        .iter()
        .filter_map(|h| host_fraction(h, &progress))
        .collect();
    let fraction = fractions.iter().sum::<f64>() / fractions.len().max(1) as f64;
    let done = hosts.iter().filter(|h| h.state == TaskState::Done).count();

    rsx! {
        div { class: "border-t border-line/60 px-4 py-2.5",
            ProgressBar { fraction }
            div { class: "mt-1.5 flex flex-wrap items-center gap-3 font-mono text-[11px] text-fog-500",
                span { "{done}/{fractions.len()} hosts done" }
            }
            if let Some(multicast) = progress.multicast {
                MulticastStats { multicast }
            }
        }
    }
}

#[component]
fn MulticastStats(multicast: MulticastProgress) -> Element {
    let rate = format_bytes(multicast.bytes_per_second as u64);
    let joined = multicast.receivers.iter().filter(|r| r.joined).count();

    rsx! {
        div { class: "mt-1 flex flex-wrap items-center gap-3 font-mono text-[11px] text-fog-500",
            span { class: "text-fog-300", "{phase_label(multicast.phase)}" }
            span { "{rate}/s" }
            span { "{joined}/{multicast.receivers.len()} joined" }
            span { "file {multicast.step}/{multicast.steps}" }
            if let Some(eta) = multicast.eta_seconds {
                span { "eta {format_duration(eta)}" }
            }
            if let Some(culprit) = multicast.receivers.iter().find(|r| r.limiting) {
                span { class: "text-warn", "held up by {culprit.name}" }
            }
        }
    }
}

#[component]
fn HostProgressStats(progress: HostProgress) -> Element {
    rsx! {
        div { class: "flex flex-wrap items-center gap-3",
            div { class: "flex w-56", ProgressBar { fraction: progress.fraction } }
            match progress.stage {
            Stage::PartitionTable => rsx! {
                span { class: "font-mono text-xs text-fog-500", "partition table" }
            },
            Stage::Partition { number, index, count, bytes_per_second, eta_seconds } => rsx! {
                span { class: "font-mono text-xs text-fog-500", "partition {number} ({index + 1}/{count})" }
                if bytes_per_second > 0.0 {
                    span { class: "font-mono text-xs text-fog-600", "{format_bytes(bytes_per_second as u64)}/s" }
                }
                if let Some(eta) = eta_seconds {
                    span { class: "font-mono text-xs text-fog-600", "eta {format_duration(eta)}" }
                }
            },
            Stage::Finishing => rsx! {
                span { class: "font-mono text-xs text-fog-500", "finishing" }
            },
            }
        }
    }
}

fn phase_label(phase: TransferPhase) -> &'static str {
    match phase {
        TransferPhase::Gathering => "waiting for hosts",
        TransferPhase::Sending => "sending",
        TransferPhase::Draining => "finishing",
    }
}

#[component]
fn ReceiverStats(receiver: ReceiverProgress) -> Element {
    let loss = receiver.loss * 100.0;
    rsx! {
        div { class: "flex flex-wrap items-center gap-3",
            span { class: "font-mono text-xs text-fog-500", "receiver" }
            if receiver.joined {
                span { class: "font-mono text-xs text-fog-600", "{receiver.slices_behind} behind" }
                span { class: "font-mono text-xs text-fog-600", "{loss:.1}% loss" }
                span { class: "font-mono text-xs text-fog-600", "{receiver.naks} naks" }
                span { class: "font-mono text-xs text-fog-600", "{receiver.sink_stall_ms} ms stalled" }
            } else {
                span { class: "font-mono text-xs text-fog-600", "not joined" }
            }
            if receiver.limiting {
                span { class: "text-xs text-warn", "holding up the transfer" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(el: Element) -> String {
        dioxus_ssr::render_element(el)
    }

    fn receiver(host_id: i64, joined: bool, limiting: bool) -> ReceiverProgress {
        ReceiverProgress {
            host_id,
            name: format!("lab-{host_id}"),
            joined,
            limiting,
            slices_behind: 3,
            loss: 0.012,
            naks: 5,
            sink_stall_ms: 40,
        }
    }

    fn multicast(eta_seconds: Option<u64>) -> MulticastProgress {
        MulticastProgress {
            phase: TransferPhase::Sending,
            bytes_per_second: 1024.0 * 1024.0,
            step: 2,
            steps: 5,
            eta_seconds,
            receivers: vec![receiver(1, true, true), receiver(2, false, false)],
        }
    }

    fn task_host(host_id: i64, state: TaskState) -> TaskHost {
        TaskHost {
            host_id,
            state,
            error: None,
            started_at: None,
            finished_at: None,
        }
    }

    fn reported(fraction: f64, stage: Stage) -> HostProgress {
        HostProgress {
            task_id: 3,
            fraction,
            stage,
        }
    }

    #[test]
    fn the_task_bar_averages_the_hosts_that_are_still_meant_to_finish() {
        let hosts = vec![
            task_host(1, TaskState::Done),
            task_host(2, TaskState::Running),
            task_host(3, TaskState::Pending),
            task_host(4, TaskState::Failed),
        ];
        let progress = TaskProgress {
            task_id: 3,
            hosts: [(2, reported(0.5, Stage::Finishing))].into(),
            multicast: None,
        };

        let html = render(rsx! { TaskProgressBar { hosts, progress } });

        assert!(html.contains("width: 50%"), "{html}");
        assert!(html.contains("1/3 hosts done"), "{html}");
        assert!(!html.contains("joined"), "{html}");
    }

    #[test]
    fn a_multicast_adds_the_transfer_stats_below_the_task_bar() {
        let progress = TaskProgress {
            task_id: 3,
            hosts: Default::default(),
            multicast: Some(multicast(Some(90))),
        };

        let html = render(
            rsx! { TaskProgressBar { hosts: vec![task_host(1, TaskState::Running)], progress } },
        );

        assert!(html.contains("width: 0%"), "{html}");
        assert!(html.contains("sending"), "{html}");
        assert!(html.contains("1.00 MiB/s"), "{html}");
        assert!(html.contains("1/2 joined"), "{html}");
        assert!(html.contains("file 2/5"), "{html}");
        assert!(html.contains("eta 1m 30s"), "{html}");
        assert!(html.contains("held up by lab-1"), "{html}");
    }

    #[test]
    fn multicast_stats_without_an_eta_omit_it() {
        let html = render(rsx! { MulticastStats { multicast: multicast(None) } });

        assert!(!html.contains("eta"), "{html}");
    }

    #[test]
    fn a_host_shows_the_partition_it_is_imaging_with_its_rate_and_eta() {
        let progress = reported(
            0.42,
            Stage::Partition {
                number: 3,
                index: 1,
                count: 2,
                bytes_per_second: 2.0 * 1024.0 * 1024.0,
                eta_seconds: Some(75),
            },
        );

        let html = render(rsx! { HostProgressStats { progress } });

        assert!(html.contains("width: 42%"), "{html}");
        assert!(html.contains("partition 3 (2/2)"), "{html}");
        assert!(html.contains("2.00 MiB/s"), "{html}");
        assert!(html.contains("eta 1m 15s"), "{html}");
    }

    #[test]
    fn a_progress_bar_is_clamped_to_full() {
        let html = render(rsx! { ProgressBar { fraction: 1.5 } });

        assert!(html.contains("width: 100%"), "{html}");
    }

    #[test]
    fn a_receiver_shows_its_stats_and_whether_it_holds_up_the_transfer() {
        let joined = render(rsx! { ReceiverStats { receiver: receiver(1, true, true) } });
        let missing = render(rsx! { ReceiverStats { receiver: receiver(2, false, false) } });

        for stat in ["3 behind", "1.2% loss", "5 naks", "40 ms stalled"] {
            assert!(joined.contains(stat), "{joined}");
        }
        assert!(joined.contains("holding up the transfer"), "{joined}");
        assert!(missing.contains("not joined"), "{missing}");
        assert!(!missing.contains("holding up"), "{missing}");
    }
}
