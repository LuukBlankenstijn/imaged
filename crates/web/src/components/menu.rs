use dioxus::prelude::*;

use crate::components::icons::Icon;

#[component]
pub fn ActionMenu(children: Element) -> Element {
    let mut open = use_signal(|| false);
    let mut trigger = use_signal(|| None as Option<std::rc::Rc<MountedData>>);
    let mut anchor = use_signal(|| (0.0_f64, 0.0_f64));
    let (right, top) = anchor();

    rsx! {
        div { class: "inline-flex",
            button {
                class: "rounded-md p-1.5 text-fog-400 hover:bg-ink-700 hover:text-fog-100 transition-colors",
                onmounted: move |e| trigger.set(Some(e.data())),
                onclick: move |e: MouseEvent| {
                    e.stop_propagation();
                    if open() {
                        open.set(false);
                        return;
                    }
                    let Some(node) = trigger() else { return };
                    spawn(async move {
                        if let Ok(rect) = node.get_client_rect().await {
                            anchor
                                .set((
                                    rect.origin.x + rect.size.width,
                                    rect.origin.y + rect.size.height,
                                ));
                            open.set(true);
                        }
                    });
                },
                Icon { name: "kebab", class: "w-4 h-4" }
            }
            if open() {
                div {
                    class: "fixed inset-0 z-40",
                    onclick: move |_| open.set(false),
                }
                div {
                    class: "fixed z-50 min-w-44 rounded-lg border border-line bg-ink-800 py-1 shadow-2xl",
                    style: "left: {right}px; top: {top + 4.0}px; transform: translateX(-100%); max-height: calc(100vh - {top + 12.0}px); overflow-y: auto;",
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
