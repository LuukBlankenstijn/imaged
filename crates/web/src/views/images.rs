use dioxus::prelude::*;

use crate::api::hosts::*;
use crate::api::images::*;
use crate::api::scripts::{
    get_all_scripts, get_image_scripts, set_image_scripts, set_root_partition,
};
use crate::components::hooks::use_poll;
use crate::components::icons::Icon;
use crate::components::modal::ConfirmDialog;
use crate::components::toast::{toast_error, toast_success};
use crate::components::ui::*;
use crate::format::{format_bytes, format_relative};
use crate::model::{
    CreateImageRequest, Image, ImageScriptsRequest, ImageStatus, RootPartitionRequest, Script,
    UpdateName,
};

#[component]
pub fn Images() -> Element {
    let mut images = use_poll(3000, || async move { get_all_images().await });
    let hosts = use_resource(move || async move { get_all_hosts().await });

    let mut new_name = use_signal(String::new);
    let mut host_id = use_signal(|| 0i64);

    let can_create = !new_name.read().trim().is_empty() && host_id() != 0;

    rsx! {
        PageHeader { title: "Images", subtitle: "Captured disk images available to deploy",
            ImportButton { on_imported: move |_| images.restart() }
        }

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
fn ImportButton(on_imported: EventHandler<()>) -> Element {
    let mut importing = use_signal(|| false);
    let classes = button_classes(ButtonVariant::Ghost);

    rsx! {
        label { class: "{classes} cursor-pointer", title: "Import an exported .imaged archive",
            if importing() {
                Spinner { class: "w-4 h-4" }
            } else {
                input {
                    r#type: "file",
                    accept: ".imaged",
                    class: "hidden",
                    onchange: move |e| {
                        let Some(file) = e.files().into_iter().next() else {
                            return;
                        };
                        importing.set(true);
                        spawn(async move {
                            match import_image(file.into()).await {
                                Ok(image) => toast_success("Image imported", image.name),
                                Err(e) => toast_error("Import failed", e.to_string()),
                            }
                            importing.set(false);
                            on_imported.call(());
                        });
                    },
                }
                Icon { name: "upload", class: "w-4 h-4" }
            }
            "Import"
        }
    }
}

#[component]
fn ImageRow(image: Image, index: usize, on_changed: EventHandler<()>) -> Element {
    let mut editing = use_signal(|| false);
    let mut confirm_open = use_signal(|| false);
    let mut setup_open = use_signal(|| false);

    let id = image.id;
    let mut image_scripts = use_resource(move || async move { get_image_scripts(id).await });
    let script_count = match &*image_scripts.read() {
        Some(Ok(list)) => list.len(),
        _ => 0,
    };
    let missing_root = script_count > 0 && image.root_partition.is_none();
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
    let export_classes = button_classes(ButtonVariant::Ghost);

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
                            if script_count > 0 {
                                span { class: "text-fog-600", "· {script_count} script(s)" }
                            }
                            if let Some(root) = image.root_partition {
                                span { class: "text-fog-600", "· root #{root}" }
                            }
                        }
                    }
                    if missing_root {
                        Badge { tone: Tone::Warn, "no root partition" }
                    }
                    ImageStatusBadge { status }
                    Button {
                        variant: if setup_open() { ButtonVariant::Primary } else { ButtonVariant::Ghost },
                        class: "px-2",
                        onclick: move |_| setup_open.toggle(),
                        Icon { name: "script", class: "w-4 h-4" }
                    }
                    if status == ImageStatus::Ready {
                        a {
                            class: "{export_classes} px-2",
                            href: "/api/ui/images/{id}/export",
                            download: "",
                            rel: "noreferrer",
                            title: "Export image",
                            Icon { name: "download", class: "w-4 h-4" }
                        }
                    }
                    Button {
                        variant: ButtonVariant::Danger,
                        class: "px-2",
                        onclick: move |_| confirm_open.set(true),
                        Icon { name: "trash", class: "w-4 h-4" }
                    }
                }
                if missing_root {
                    div { class: "flex items-start gap-2 border-t border-warn/20 bg-warn/5 px-4 py-2 font-mono text-xs text-warn",
                        Icon { name: "alert", class: "mt-0.5 w-3.5 h-3.5 shrink-0" }
                        span { "Post-install scripts are attached but no root partition is set: deploys of this image are refused until one is selected." }
                    }
                }
                if setup_open() {
                    div { class: "grid gap-6 border-t border-line/60 bg-ink-900/40 px-4 py-4 md:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]",
                        RootPartitionPicker { image: image.clone(), on_changed }
                        match &*image_scripts.read() {
                            Some(Ok(current)) => rsx! {
                                ImageScriptList {
                                    image_id: id,
                                    current: current.clone(),
                                    on_saved: move |_| image_scripts.restart(),
                                }
                            },
                            Some(Err(e)) => rsx! {
                                p { class: "text-sm text-bad", "Failed to load scripts: {e}" }
                            },
                            None => rsx! {
                                Spinner {}
                            },
                        }
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

#[component]
fn RootPartitionPicker(image: Image, on_changed: EventHandler<()>) -> Element {
    let image_id = image.id;
    let saved = image.root_partition;
    let mut shown = use_signal(|| saved);
    use_effect(use_reactive!(|saved| shown.set(saved)));
    let current = shown().map(|n| n.to_string()).unwrap_or_default();

    rsx! {
        div {
            div { class: "mb-1 font-mono text-[11px] uppercase tracking-wider text-fog-500",
                "Root partition"
            }
            if image.partitions.is_empty() {
                p { class: "text-xs text-fog-600", "No partitions captured yet." }
            } else {
                select {
                    class: "{FIELD_CLASSES} w-full font-mono",
                    value: "{current}",
                    onchange: move |e| {
                        let partition_number = e.value().parse::<i64>().ok();
                        shown.set(partition_number);
                        spawn(async move {
                            match set_root_partition(RootPartitionRequest { image_id, partition_number }).await {
                                Ok(_) => on_changed.call(()),
                                Err(err) => {
                                    shown.set(saved);
                                    toast_error("Root partition not saved", err.to_string());
                                }
                            }
                        });
                    },
                    option { value: "", selected: shown().is_none(), "none" }
                    for p in image.partitions.iter() {
                        option {
                            key: "{p.partition_number}",
                            value: "{p.partition_number}",
                            selected: shown() == Some(p.partition_number),
                            {format!("#{} · {} · {}", p.partition_number, p.fstype, format_bytes(p.size_bytes))}
                        }
                    }
                }
            }
            p { class: "mt-1.5 text-xs text-fog-600",
                "Post-install scripts run chrooted in this partition after the image is restored."
            }
        }
    }
}

#[component]
fn ImageScriptList(image_id: i64, current: Vec<Script>, on_saved: EventHandler<()>) -> Element {
    let all_scripts = use_resource(|| async move { get_all_scripts().await });
    let mut draft = use_signal({
        let seed = current.clone();
        move || seed
    });
    let mut saving = use_signal(|| false);

    let list = draft();
    let count = list.len();
    let ids: Vec<i64> = list.iter().map(|s| s.id).collect();
    let current_ids: Vec<i64> = current.iter().map(|s| s.id).collect();
    let dirty = ids != current_ids;
    let available: Vec<Script> = match &*all_scripts.read() {
        Some(Ok(all)) => all
            .iter()
            .filter(|s| !ids.contains(&s.id))
            .cloned()
            .collect(),
        _ => Vec::new(),
    };

    rsx! {
        div {
            div { class: "mb-1 font-mono text-[11px] uppercase tracking-wider text-fog-500",
                "Post-install scripts"
            }
            if list.is_empty() {
                p { class: "rounded-md border border-line bg-ink-900/60 px-3 py-2 text-xs text-fog-600",
                    "No scripts run after deploying this image."
                }
            } else {
                div { class: "flex flex-col divide-y divide-line/60 rounded-md border border-line",
                    for (index , script) in list.iter().enumerate() {
                        div { key: "{script.id}", class: "flex items-center gap-3 px-3 py-1.5",
                            span { class: "w-5 font-mono text-xs text-fog-600", "{index + 1}" }
                            span { class: "min-w-0 flex-1 truncate font-mono text-sm text-fog-100", "{script.name}" }
                            span { class: "font-mono text-[11px] text-fog-600", "{script.inputs.len()} input(s)" }
                            Button {
                                variant: ButtonVariant::Subtle,
                                class: "px-1.5",
                                disabled: index == 0,
                                onclick: move |_| draft.write().swap(index, index - 1),
                                Icon { name: "chevron", class: "w-3.5 h-3.5 rotate-180" }
                            }
                            Button {
                                variant: ButtonVariant::Subtle,
                                class: "px-1.5",
                                disabled: index + 1 == count,
                                onclick: move |_| draft.write().swap(index, index + 1),
                                Icon { name: "chevron", class: "w-3.5 h-3.5" }
                            }
                            Button {
                                variant: ButtonVariant::Subtle,
                                class: "px-1.5 hover:text-bad",
                                onclick: move |_| {
                                    draft.write().remove(index);
                                },
                                Icon { name: "close", class: "w-3.5 h-3.5" }
                            }
                        }
                    }
                }
            }
            div { class: "mt-2 flex flex-wrap items-center gap-2",
                select {
                    class: "{FIELD_CLASSES} min-w-48 flex-1 font-mono",
                    value: "",
                    disabled: available.is_empty(),
                    onchange: {
                        let available = available.clone();
                        move |e: FormEvent| {
                            let picked = e.value().parse::<i64>().ok();
                            if let Some(script) = available.iter().find(|s| Some(s.id) == picked) {
                                draft.write().push(script.clone());
                            }
                        }
                    },
                    option { value: "", selected: true, "add script…" }
                    for script in available.iter() {
                        option { key: "{script.id}", value: "{script.id}", "{script.name}" }
                    }
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    disabled: !dirty || saving(),
                    onclick: move |_| draft.set(current.clone()),
                    "Reset"
                }
                Button {
                    disabled: !dirty || saving(),
                    onclick: move |_| {
                        let script_ids = ids.clone();
                        saving.set(true);
                        spawn(async move {
                            match set_image_scripts(ImageScriptsRequest { image_id, script_ids }).await {
                                Ok(_) => on_saved.call(()),
                                Err(err) => toast_error("Scripts not saved", err.to_string()),
                            }
                            saving.set(false);
                        });
                    },
                    "Save"
                }
            }
        }
    }
}
