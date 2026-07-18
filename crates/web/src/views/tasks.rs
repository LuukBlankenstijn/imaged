use dioxus::prelude::*;

use crate::api::tasks::{cancel_task, get_all_tasks, retry_task};
use crate::components::hooks::use_poll;
use crate::components::icons::Icon;
use crate::components::modal::ConfirmDialog;
use crate::components::toast::toast_error;
use crate::components::ui::{
    Button, ButtonVariant, Card, EmptyState, PageHeader, Spinner, TaskStateBadge, TaskTypeBadge,
};
use crate::format::format_relative;
use crate::model::{Task, TaskState, TaskType};

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
                    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
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
    let needs_confirm = matches!(task.r#type, TaskType::Deploy | TaskType::Multicast);

    let image_label = task.image_name.clone().unwrap_or_else(|| "\u{2014}".to_string());
    let host_count = task.hosts.len();
    let hosts = task.hosts.clone();

    let do_cancel = move || {
        spawn(async move {
            match cancel_task(task_id).await {
                Ok(_) => on_changed.call(()),
                Err(err) => toast_error("Cancel failed", err.to_string()),
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
                                if needs_confirm {
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
            if expanded() {
                div { class: "border-t border-line/60 px-4 py-3",
                    div { class: "flex flex-col gap-2",
                        for h in hosts.iter() {
                            div { key: "{h.host_id}", class: "flex flex-wrap items-center gap-3 text-sm",
                                span { class: "font-mono text-xs text-fog-500", "host {h.host_id}" }
                                TaskStateBadge { state: h.state }
                                if let Some(started) = h.started_at {
                                    span { class: "font-mono text-xs text-fog-600", "start {format_relative(started)}" }
                                }
                                if let Some(finished) = h.finished_at {
                                    span { class: "font-mono text-xs text-fog-600", "end {format_relative(finished)}" }
                                }
                                if let Some(err) = h.error.clone() {
                                    span { class: "text-xs text-bad", "{err}" }
                                }
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
        }
    }
}
