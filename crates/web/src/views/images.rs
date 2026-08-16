use dioxus::prelude::*;

use crate::api::hosts::*;
use crate::api::images::*;
use crate::components::hooks::use_poll;
use crate::components::icons::Icon;
use crate::components::modal::ConfirmDialog;
use crate::components::toast::toast_error;
use crate::components::ui::*;
use crate::format::{format_bytes, format_relative};
use crate::model::{CreateImageRequest, Image, ImageStatus, UpdateName};

#[component]
pub fn Images() -> Element {
    let mut images = use_poll(3000, || async move { get_all_images().await });
    let hosts = use_resource(move || async move { get_all_hosts().await });

    let mut new_name = use_signal(String::new);
    let mut host_id = use_signal(|| 0i64);

    let can_create = !new_name.read().trim().is_empty() && host_id() != 0;

    rsx! {
        PageHeader { title: "Images", subtitle: "Captured disk images available to deploy" }

        Card { class: "mb-6 p-4",
            div { class: "flex flex-wrap items-end gap-3",
                div { class: "min-w-52 flex-1",
                    label { class: "mb-1 block font-mono text-[11px] uppercase tracking-wider text-fog-500",
                        "Image name"
                    }
                    input {
                        class: "w-full rounded-md border border-ink-600 bg-ink-900/60 px-3 py-1.5 font-mono text-sm text-fog-100 placeholder:text-fog-600 focus:border-amber-500/60 focus:outline-none focus-visible:glow-amber",
                        placeholder: "golden-image-01",
                        value: "{new_name}",
                        oninput: move |e| new_name.set(e.value()),
                    }
                }
                div { class: "min-w-48",
                    label { class: "mb-1 block font-mono text-[11px] uppercase tracking-wider text-fog-500",
                        "Source host"
                    }
                    div { class: "relative",
                        select {
                            class: "w-full appearance-none rounded-md border border-ink-600 bg-ink-900/60 px-3 py-1.5 pr-9 font-mono text-sm text-fog-100 focus:border-amber-500/60 focus:outline-none focus-visible:glow-amber",
                            value: "{host_id}",
                            onchange: move |e| host_id.set(e.value().parse().unwrap_or(0)),
                            option { value: "0", disabled: true, "select host…" }
                            {
                                match &*hosts.read() {
                                    Some(Ok(list)) => rsx! {
                                        for h in list.iter() {
                                            option { key: "{h.id}", value: "{h.id}", "{h.name}" }
                                        }
                                    },
                                    _ => rsx! {},
                                }
                            }
                        }
                        div { class: "pointer-events-none absolute inset-y-0 right-2 flex items-center text-fog-500",
                            Icon { name: "chevron", class: "w-4 h-4" }
                        }
                    }
                }
                Button {
                    variant: ButtonVariant::Primary,
                    disabled: !can_create,
                    onclick: move |_| {
                        let name = new_name.read().trim().to_string();
                        let hid = host_id();
                        if name.is_empty() || hid == 0 {
                            return;
                        }
                        let mut images = images;
                        let mut new_name = new_name;
                        let mut host_id = host_id;
                        spawn(async move {
                            match create_image(CreateImageRequest { name, host_id: hid }).await {
                                Ok(_) => {
                                    new_name.set(String::new());
                                    host_id.set(0);
                                    images.restart();
                                }
                                Err(e) => toast_error("Capture failed", e.to_string()),
                            }
                        });
                    },
                    Icon { name: "capture", class: "w-4 h-4" }
                    "Capture"
                }
            }
        }

        {
            match &*images.read() {
                Some(Ok(list)) if list.is_empty() => rsx! {
                    Card {
                        EmptyState {
                            title: "No images captured",
                            hint: "Capture a disk image from a source host to build the deploy library.",
                        }
                    }
                },
                Some(Ok(list)) => rsx! {
                    div { class: "flex flex-col gap-3",
                        for (i , img) in list.iter().enumerate() {
                            ImageRow {
                                key: "{img.id}",
                                image: img.clone(),
                                index: i,
                                on_changed: move |_| images.restart(),
                            }
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    Card {
                        EmptyState { title: "Failed to load images", hint: e.to_string() }
                    }
                },
                None => rsx! {
                    div { class: "flex justify-center py-16", Spinner {} }
                },
            }
        }
    }
}

#[component]
fn ImageRow(image: Image, index: usize, on_changed: EventHandler<()>) -> Element {
    let mut editing = use_signal(|| false);
    let mut confirm_open = use_signal(|| false);

    let id = image.id;
    let status = image.status;
    let name = image.name.clone();
    let total: u64 = image.partitions.iter().map(|p| p.size_bytes).sum();
    let size = format_bytes(total);
    let captured_rel = image.captured_at.map(format_relative);
    let error_message = image.error_message.clone();

    let mut draft = use_signal({
        let seed = name.clone();
        move || seed
    });
    let name_for_edit = name.clone();
    let name_for_msg = name.clone();

    let delay = index as u32 * 45;
    let faulted = status == ImageStatus::Faulted;

    rsx! {
        div { class: "rise", style: "--d: {delay}ms",
            Card { class: "overflow-hidden",
                div { class: "flex items-center gap-4 px-4 py-3",
                    div { class: "flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-line bg-ink-900/60 text-amber-500",
                        Icon { name: "image", class: "w-4 h-4" }
                    }
                    div { class: "min-w-0 flex-1",
                        if editing() {
                            div { class: "flex items-center gap-2",
                                input {
                                    class: "min-w-0 flex-1 rounded-md border border-amber-500/50 bg-ink-900/60 px-2 py-1 font-mono text-sm text-fog-100 focus:outline-none focus-visible:glow-amber",
                                    value: "{draft}",
                                    oninput: move |e| draft.set(e.value()),
                                }
                                Button {
                                    variant: ButtonVariant::Subtle,
                                    class: "px-1.5",
                                    onclick: move |_| {
                                        let mut editing = editing;
                                        let new_name = draft.read().trim().to_string();
                                        if new_name.is_empty() {
                                            editing.set(false);
                                            return;
                                        }
                                        spawn(async move {
                                            match update_image_name(UpdateName { id, new_name }).await {
                                                Ok(_) => {
                                                    editing.set(false);
                                                    on_changed.call(());
                                                }
                                                Err(e) => toast_error("Rename failed", e.to_string()),
                                            }
                                        });
                                    },
                                    Icon { name: "check", class: "w-4 h-4 text-ok" }
                                }
                                Button {
                                    variant: ButtonVariant::Subtle,
                                    class: "px-1.5",
                                    onclick: move |_| editing.set(false),
                                    Icon { name: "close", class: "w-4 h-4" }
                                }
                            }
                        } else {
                            div { class: "flex items-center gap-2",
                                span { class: "truncate font-display text-fog-100", "{name}" }
                                button {
                                    class: "text-fog-600 transition-colors hover:text-amber-400",
                                    onclick: move |_| {
                                        editing.set(true);
                                        draft.set(name_for_edit.clone());
                                    },
                                    Icon { name: "edit", class: "w-3.5 h-3.5" }
                                }
                            }
                        }
                        div { class: "mt-0.5 flex flex-wrap items-center gap-x-3 font-mono text-xs text-fog-500",
                            span { "{size}" }
                            if let Some(rel) = captured_rel {
                                span { class: "text-fog-600", "· captured {rel}" }
                            }
                        }
                    }
                    ImageStatusBadge { status }
                    Button {
                        variant: ButtonVariant::Danger,
                        class: "px-2",
                        onclick: move |_| confirm_open.set(true),
                        Icon { name: "trash", class: "w-4 h-4" }
                    }
                }
                if faulted {
                    if let Some(msg) = error_message {
                        div { class: "flex items-start gap-2 border-t border-bad/20 bg-bad/5 px-4 py-2 font-mono text-xs text-bad",
                            Icon { name: "alert", class: "mt-0.5 w-3.5 h-3.5 shrink-0" }
                            span { class: "break-words", "{msg}" }
                        }
                    }
                }
            }
        }
        ConfirmDialog {
            open: confirm_open(),
            title: "Delete image",
            message: "Permanently delete \"{name_for_msg}\" and its captured data? Active tasks must be cancelled first.",
            confirm_label: "Delete",
            danger: true,
            onconfirm: move |_| {
                let mut confirm_open = confirm_open;
                confirm_open.set(false);
                spawn(async move {
                    match delete_image(id).await {
                        Ok(_) => on_changed.call(()),
                        Err(e) => toast_error("Delete failed", e.to_string()),
                    }
                });
            },
            oncancel: move |_| confirm_open.set(false),
        }
    }
}
