
use dioxus::prelude::*;

use crate::components::icons::Icon;
use crate::components::ui::{Button, ButtonVariant};

#[component]
pub fn Modal(
    open: bool,
    onclose: EventHandler<()>,
    #[props(default)] title: String,
    children: Element,
) -> Element {
    if !open {
        return rsx! {};
    }
    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center p-4",
            div {
                class: "absolute inset-0 bg-ink-950/75 backdrop-blur-sm",
                onclick: move |_| onclose.call(()),
            }
            div { class: "relative z-10 w-full max-w-lg rounded-xl border border-line bg-ink-850 shadow-2xl rise",
                if !title.is_empty() {
                    div { class: "flex items-center justify-between border-b border-line px-5 py-3",
                        h3 { class: "font-display text-lg text-fog-100", "{title}" }
                        button {
                            class: "text-fog-500 hover:text-fog-100",
                            onclick: move |_| onclose.call(()),
                            Icon { name: "close" }
                        }
                    }
                }
                div { class: "p-5", {children} }
            }
        }
    }
}

#[component]
pub fn ConfirmDialog(
    open: bool,
    title: String,
    message: String,
    #[props(default = String::from("Confirm"))] confirm_label: String,
    #[props(default)] danger: bool,
    onconfirm: EventHandler<()>,
    oncancel: EventHandler<()>,
) -> Element {
    let variant = if danger {
        ButtonVariant::Danger
    } else {
        ButtonVariant::Primary
    };
    rsx! {
        Modal { open, onclose: move |_| oncancel.call(()), title,
            p { class: "text-sm leading-relaxed text-fog-400", "{message}" }
            div { class: "mt-6 flex justify-end gap-2",
                Button { variant: ButtonVariant::Ghost, onclick: move |_| oncancel.call(()), "Cancel" }
                Button { variant, onclick: move |_| onconfirm.call(()), "{confirm_label}" }
            }
        }
    }
}
