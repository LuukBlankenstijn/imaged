
use dioxus::prelude::*;

use crate::components::icons::Icon;

#[component]
pub fn ActionMenu(children: Element) -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        div { class: "relative inline-flex",
            button {
                class: "rounded-md p-1.5 text-fog-400 hover:bg-ink-700 hover:text-fog-100 transition-colors",
                onclick: move |e: MouseEvent| {
                    e.stop_propagation();
                    open.toggle();
                },
                Icon { name: "kebab", class: "w-4 h-4" }
            }
            if open() {
                div {
                    class: "fixed inset-0 z-40",
                    onclick: move |_| open.set(false),
                }
                div {
                    class: "absolute right-0 top-full mt-1 z-50 min-w-44 overflow-hidden rounded-lg border border-line bg-ink-800 py-1 shadow-2xl rise",
                    onclick: move |_| open.set(false),
                    {children}
                }
            }
        }
    }
}

#[component]
pub fn MenuItem(
    label: String,
    onclick: EventHandler<MouseEvent>,
    #[props(default)] icon: &'static str,
    #[props(default)] danger: bool,
    #[props(default)] disabled: bool,
) -> Element {
    let tone = if disabled {
        "text-fog-600 cursor-not-allowed"
    } else if danger {
        "text-bad hover:bg-bad/10"
    } else {
        "text-fog-300 hover:bg-ink-700 hover:text-fog-100"
    };
    rsx! {
        button {
            class: "flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-sm transition-colors {tone}",
            disabled,
            onclick: move |e| onclick.call(e),
            if !icon.is_empty() {
                Icon { name: icon, class: "w-3.5 h-3.5 opacity-80" }
            }
            "{label}"
        }
    }
}
