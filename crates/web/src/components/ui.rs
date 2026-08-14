use dioxus::prelude::*;

use crate::components::icons::Icon;
use crate::model::{ImageStatus, TaskState, TaskType};

#[derive(Clone, Copy, PartialEq, Default)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Ghost,
    Danger,
    Subtle,
}

#[component]
pub fn Button(
    #[props(default)] variant: ButtonVariant,
    #[props(default)] disabled: bool,
    #[props(default)] onclick: EventHandler<MouseEvent>,
    #[props(default)] class: String,
    children: Element,
) -> Element {
    let base = "inline-flex items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-sm \
                font-medium transition-colors focus:outline-none focus-visible:glow-amber \
                disabled:opacity-40 disabled:cursor-not-allowed";
    let v = match variant {
        ButtonVariant::Primary => "bg-amber-500 text-ink-950 hover:bg-amber-400 font-semibold",
        ButtonVariant::Ghost => {
            "border border-ink-600 text-fog-300 hover:bg-ink-700 hover:text-fog-100"
        }
        ButtonVariant::Danger => "border border-bad/40 text-bad hover:bg-bad/10",
        ButtonVariant::Subtle => "text-fog-400 hover:text-fog-100 hover:bg-ink-700",
    };
    rsx! {
        button {
            class: "{base} {v} {class}",
            disabled,
            onclick: move |e| onclick.call(e),
            {children}
        }
    }
}

#[component]
pub fn Card(
    #[props(default)] class: String,
    #[props(default)] style: String,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "rounded-xl border border-line bg-ink-850/80 {class}",
            style,
            {children}
        }
    }
}

#[component]
pub fn PageHeader(
    title: String,
    #[props(default)] subtitle: String,
    #[props(default)] children: Element,
) -> Element {
    rsx! {
        header { class: "flex flex-wrap items-end justify-between gap-4 mb-6",
            div {
                h1 { class: "text-2xl font-display font-semibold text-fog-100 tracking-wide", "{title}" }
                if !subtitle.is_empty() {
                    p { class: "text-sm text-fog-500 mt-1", "{subtitle}" }
                }
            }
            div { class: "flex items-center gap-2", {children} }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Ok,
    Run,
    Warn,
    Bad,
    Idle,
    Neutral,
}

fn tone_classes(t: Tone) -> &'static str {
    match t {
        Tone::Ok => "text-ok border-ok/30 bg-ok/10",
        Tone::Run => "text-run border-run/30 bg-run/10",
        Tone::Warn => "text-warn border-warn/30 bg-warn/10",
        Tone::Bad => "text-bad border-bad/30 bg-bad/10",
        Tone::Idle => "text-idle border-idle/30 bg-idle/10",
        Tone::Neutral => "text-fog-400 border-line bg-ink-700/60",
    }
}

#[component]
pub fn Badge(tone: Tone, #[props(default)] class: String, children: Element) -> Element {
    let tc = tone_classes(tone);
    rsx! {
        span {
            class: "inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-mono uppercase tracking-wider {tc} {class}",
            {children}
        }
    }
}

#[component]
pub fn ImageStatusBadge(status: ImageStatus) -> Element {
    let (tone, label) = match status {
        ImageStatus::Ready => (Tone::Ok, "ready"),
        ImageStatus::Capturing => (Tone::Warn, "capturing"),
        ImageStatus::Faulted => (Tone::Bad, "faulted"),
        ImageStatus::Empty => (Tone::Idle, "empty"),
    };
    rsx! {
        Badge { tone, "{label}" }
    }
}

#[component]
pub fn TaskStateBadge(state: TaskState) -> Element {
    let (tone, label) = match state {
        TaskState::Done => (Tone::Ok, "done"),
        TaskState::Running => (Tone::Run, "running"),
        TaskState::Pending => (Tone::Warn, "pending"),
        TaskState::Failed => (Tone::Bad, "failed"),
        TaskState::Partial => (Tone::Warn, "partial"),
        TaskState::Cancelled => (Tone::Idle, "cancelled"),
    };
    rsx! {
        Badge { tone, "{label}" }
    }
}

#[component]
pub fn TaskTypeBadge(r#type: TaskType) -> Element {
    let (icon, label) = match r#type {
        TaskType::Capture => ("capture", "capture"),
        TaskType::Deploy => ("deploy", "deploy"),
        TaskType::Multicast => ("multicast", "multicast"),
        TaskType::Reboot => ("reboot", "reboot"),
    };
    rsx! {
        Badge { tone: Tone::Neutral,
            Icon { name: icon, class: "w-3 h-3" }
            "{label}"
        }
    }
}

#[component]
pub fn StatusDot(connected: bool) -> Element {
    if connected {
        rsx! {
            span { class: "inline-block w-2.5 h-2.5 rounded-full bg-ok pulse-live", title: "connected" }
        }
    } else {
        rsx! {
            span { class: "inline-block w-2.5 h-2.5 rounded-full bg-idle/50", title: "disconnected" }
        }
    }
}

#[component]
pub fn Spinner(#[props(default)] class: String) -> Element {
    let class = if class.is_empty() {
        "w-5 h-5".to_string()
    } else {
        class
    };
    rsx! {
        svg {
            class: "{class} animate-spin text-amber-500",
            "viewBox": "0 0 24 24",
            fill: "none",
            circle {
                class: "opacity-25",
                cx: "12",
                cy: "12",
                r: "10",
                stroke: "currentColor",
                "stroke-width": "3",
            }
            path {
                class: "opacity-90",
                fill: "currentColor",
                d: "M12 2a10 10 0 0 1 10 10h-3a7 7 0 0 0-7-7z",
            }
        }
    }
}

#[component]
pub fn EmptyState(title: String, #[props(default)] hint: String) -> Element {
    rsx! {
        div { class: "flex flex-col items-center justify-center text-center py-16",
            div { class: "font-display text-lg text-fog-300", "{title}" }
            if !hint.is_empty() {
                p { class: "text-sm mt-1 text-fog-600", "{hint}" }
            }
        }
    }
}
