use dioxus::prelude::*;

use crate::components::icons::Icon;

#[derive(Clone, Copy, PartialEq)]
pub enum ToastTone {
    Error,
    Success,
}

#[derive(Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub tone: ToastTone,
    pub title: String,
    pub message: String,
}

static TOASTS: GlobalSignal<Vec<Toast>> = Signal::global(Vec::new);
static NEXT_ID: GlobalSignal<u64> = Signal::global(|| 0);

fn push(tone: ToastTone, title: String, message: String) {
    let id = {
        let mut n = NEXT_ID.write();
        *n += 1;
        *n
    };
    TOASTS.write().push(Toast {
        id,
        tone,
        title,
        message,
    });
    #[cfg(feature = "web")]
    spawn(async move {
        gloo_timers::future::TimeoutFuture::new(6000).await;
        TOASTS.write().retain(|t| t.id != id);
    });
}

pub fn toast_error(title: impl Into<String>, message: impl Into<String>) {
    push(ToastTone::Error, title.into(), message.into());
}

pub fn toast_success(title: impl Into<String>, message: impl Into<String>) {
    push(ToastTone::Success, title.into(), message.into());
}

#[component]
pub fn Toaster() -> Element {
    let toasts = TOASTS();
    rsx! {
        div {
            class: "fixed bottom-4 right-4 z-[60] flex flex-col gap-2 w-80 max-w-[calc(100vw-2rem)]",
            for t in toasts.iter() {
                ToastCard { key: "{t.id}", toast: t.clone() }
            }
        }
    }
}

#[component]
fn ToastCard(toast: Toast) -> Element {
    let (accent, icon) = match toast.tone {
        ToastTone::Error => ("border-l-bad", "alert"),
        ToastTone::Success => ("border-l-ok", "check"),
    };
    let id = toast.id;
    rsx! {
        div {
            class: "rise flex items-start gap-3 rounded-lg border border-line border-l-4 {accent} bg-ink-800/95 px-3 py-2.5 shadow-xl",
            div { class: "mt-0.5 text-fog-400", Icon { name: icon, class: "w-4 h-4" } }
            div { class: "min-w-0 flex-1",
                div { class: "text-sm font-medium text-fog-100", "{toast.title}" }
                if !toast.message.is_empty() {
                    div { class: "text-xs text-fog-500 mt-0.5 break-words", "{toast.message}" }
                }
            }
            button {
                class: "text-fog-600 hover:text-fog-200",
                onclick: move |_| { TOASTS.write().retain(|t| t.id != id); },
                Icon { name: "close", class: "w-3.5 h-3.5" }
            }
        }
    }
}
