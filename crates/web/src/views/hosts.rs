use dioxus::prelude::*;

use crate::api::hosts::{
    delete_host, deploy, get_all_hosts, reboot, update_host_name, wake_on_lan,
};
use crate::api::images::get_all_images;
use crate::components::connection::use_connection;
use crate::components::hooks::use_poll;
use crate::components::icons::Icon;
use crate::components::menu::{ActionMenu, MenuItem};
use crate::components::modal::{ConfirmDialog, Modal};
use crate::components::toast::{toast_error, toast_success};
use crate::components::ui::{
    Button, ButtonVariant, Card, EmptyState, PageHeader, Spinner, StatusDot,
};
use crate::format::format_bytes;
use crate::model::{DeployRequest, Host, Image, ImageStatus, UpdateName};

#[component]
pub fn Hosts() -> Element {
    let mut hosts = use_poll(4000, || async move { get_all_hosts().await });

    rsx! {
        PageHeader { title: "Hosts", subtitle: "Registered machines on the netboot fabric" }
        Card { class: "overflow-x-auto",
            {
                match &*hosts.read() {
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        EmptyState {
                            title: "No hosts detected",
                            hint: "Machines appear here once they PXE-boot and register.",
                        }
                    },
                    Some(Ok(list)) => rsx! {
                        table { class: "w-full text-sm",
                            thead {
                                tr { class: "text-fog-500",
                                    th { class: "px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider w-10" }
                                    th { class: "px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider", "Host" }
                                    th { class: "px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider", "MAC address" }
                                    th { class: "px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider", "IP" }
                                    th { class: "px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider", "Disk" }
                                    th { class: "px-4 py-2.5 text-right text-xs font-medium uppercase tracking-wider" }
                                }
                            }
                            tbody {
                                for (i , h) in list.iter().enumerate() {
                                    HostRow {
                                        key: "{h.id}",
                                        host: h.clone(),
                                        index: i,
                                        on_changed: move |_| hosts.restart(),
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        EmptyState { title: "Failed to load hosts", hint: e.to_string() }
                    },
                    None => rsx! {
                        div { class: "flex justify-center py-16", Spinner {} }
                    },
                }
            }
        }
    }
}

#[component]
fn HostRow(host: Host, index: usize, on_changed: EventHandler<()>) -> Element {
    let host_id = host.id;
    let name = host.name.clone();
    let ip_display = host.ip.clone().unwrap_or_else(|| "\u{2014}".to_string());
    let disk = format_bytes(host.disk_size_bytes);
    let connected = use_connection(host_id);
    let delay = index * 40;

    let mut editing = use_signal(|| false);
    let mut draft = use_signal(|| String::new());
    let mut deploy_open = use_signal(|| false);
    let mut reboot_open = use_signal(|| false);
    let mut delete_open = use_signal(|| false);

    let edit_name = name.clone();
    let wake_name = name.clone();

    rsx! {
        tr {
            class: "group border-t border-line/60 transition-colors hover:bg-ink-800/40 rise",
            style: "--d: {delay}ms",
            td { class: "px-4 py-3 w-10", StatusDot { connected } }
            td { class: "px-4 py-3",
                if editing() {
                    div { class: "flex items-center gap-1.5",
                        input {
                            class: "w-44 rounded-md border border-ink-600 bg-ink-900 px-2 py-1 text-sm text-fog-100 focus:outline-none focus-visible:glow-amber",
                            value: "{draft}",
                            autofocus: true,
                            oninput: move |e| draft.set(e.value()),
                            onkeydown: move |e| {
                                match e.key() {
                                    Key::Enter => {
                                        let new_name = draft().trim().to_string();
                                        editing.set(false);
                                        if !new_name.is_empty() {
                                            spawn(async move {
                                                match update_host_name(UpdateName { id: host_id, new_name }).await {
                                                    Ok(_) => on_changed.call(()),
                                                    Err(err) => toast_error("Rename failed", err.to_string()),
                                                }
                                            });
                                        }
                                    }
                                    Key::Escape => editing.set(false),
                                    _ => {}
                                }
                            },
                        }
                        button {
                            class: "rounded-md p-1.5 text-ok transition-colors hover:bg-ok/10",
                            title: "Save",
                            onclick: move |_| {
                                let new_name = draft().trim().to_string();
                                editing.set(false);
                                if !new_name.is_empty() {
                                    spawn(async move {
                                        match update_host_name(UpdateName { id: host_id, new_name }).await {
                                            Ok(_) => on_changed.call(()),
                                            Err(err) => toast_error("Rename failed", err.to_string()),
                                        }
                                    });
                                }
                            },
                            Icon { name: "check", class: "w-4 h-4" }
                        }
                        button {
                            class: "rounded-md p-1.5 text-fog-400 transition-colors hover:bg-ink-700 hover:text-fog-100",
                            title: "Cancel",
                            onclick: move |_| editing.set(false),
                            Icon { name: "close", class: "w-4 h-4" }
                        }
                    }
                } else {
                    span { class: "font-medium text-fog-100", "{name}" }
                }
            }
            td { class: "px-4 py-3 font-mono text-xs text-fog-400", "{host.mac_address}" }
            td { class: "px-4 py-3 font-mono text-xs text-fog-400", "{ip_display}" }
            td { class: "px-4 py-3 font-mono text-xs text-fog-300", "{disk}" }
            td { class: "px-4 py-3 text-right",
                ActionMenu {
                    MenuItem {
                        label: "Deploy",
                        icon: "deploy",
                        onclick: move |_| deploy_open.set(true),
                    }
                    MenuItem {
                        label: "Rename",
                        icon: "edit",
                        onclick: move |_| {
                            draft.set(edit_name.clone());
                            editing.set(true);
                        },
                    }
                    MenuItem {
                        label: "Reboot",
                        icon: "reboot",
                        onclick: move |_| reboot_open.set(true),
                    }
                    MenuItem {
                        label: "Wake",
                        icon: "wake",
                        onclick: move |_| {
                            let host_name = wake_name.clone();
                            spawn(async move {
                                match wake_on_lan(vec![host_id]).await {
                                    Ok(_) => toast_success("Wake sent", host_name),
                                    Err(err) => toast_error("Wake failed", err.to_string()),
                                }
                            });
                        },
                    }
                    MenuItem {
                        label: "Delete",
                        icon: "trash",
                        danger: true,
                        onclick: move |_| delete_open.set(true),
                    }
                }
                if deploy_open() {
                    DeployModal {
                        host: host.clone(),
                        onclose: move |_| deploy_open.set(false),
                        on_deployed: move |_| {
                            deploy_open.set(false);
                            on_changed.call(());
                        },
                    }
                }
                ConfirmDialog {
                    open: reboot_open(),
                    title: "Reboot host",
                    message: format!("Reboot {name}? The machine will restart into the netboot environment."),
                    confirm_label: "Reboot",
                    onconfirm: move |_| {
                        reboot_open.set(false);
                        spawn(async move {
                            match reboot(vec![host_id]).await {
                                Ok(_) => on_changed.call(()),
                                Err(err) => toast_error("Reboot failed", err.to_string()),
                            }
                        });
                    },
                    oncancel: move |_| reboot_open.set(false),
                }
                ConfirmDialog {
                    open: delete_open(),
                    title: "Delete host",
                    message: format!("Remove {name} from the registry? This cannot be undone."),
                    confirm_label: "Delete",
                    danger: true,
                    onconfirm: move |_| {
                        delete_open.set(false);
                        spawn(async move {
                            match delete_host(host_id).await {
                                Ok(_) => on_changed.call(()),
                                Err(err) => toast_error("Delete failed", err.to_string()),
                            }
                        });
                    },
                    oncancel: move |_| delete_open.set(false),
                }
            }
        }
    }
}

#[component]
fn DeployModal(host: Host, onclose: EventHandler<()>, on_deployed: EventHandler<()>) -> Element {
    let host_id = host.id;
    let host_name = host.name.clone();
    let images = use_resource(|| async move { get_all_images().await });
    let mut selected = use_signal(|| None::<i64>);

    rsx! {
        Modal { open: true, onclose: move |_| onclose.call(()), title: "Deploy image",
            p { class: "mb-4 text-sm text-fog-400",
                "Select a ready image to deploy to "
                span { class: "font-medium text-fog-100", "{host_name}" }
                "."
            }
            {
                match &*images.read() {
                    Some(Ok(list)) => {
                        let ready: Vec<Image> = list
                            .iter()
                            .filter(|i| i.status == ImageStatus::Ready)
                            .cloned()
                            .collect();
                        if ready.is_empty() {
                            rsx! {
                                EmptyState {
                                    title: "No ready images",
                                    hint: "Capture an image and wait for it to finish first.",
                                }
                            }
                        } else {
                            let default_id = ready.first().map(|i| i.id);
                            rsx! {
                                select {
                                    class: "w-full rounded-md border border-ink-600 bg-ink-900 px-3 py-2 text-sm text-fog-100 focus:outline-none focus-visible:glow-amber",
                                    onchange: move |e| selected.set(e.value().parse::<i64>().ok()),
                                    for img in ready.iter() {
                                        option { key: "{img.id}", value: "{img.id}", "{img.name}" }
                                    }
                                }
                                div { class: "mt-6 flex justify-end gap-2",
                                    Button {
                                        variant: ButtonVariant::Ghost,
                                        onclick: move |_| onclose.call(()),
                                        "Cancel"
                                    }
                                    Button {
                                        onclick: move |_| {
                                            let image_id = selected().or(default_id);
                                            spawn(async move {
                                                let Some(image_id) = image_id else {
                                                    return;
                                                };
                                                match deploy(DeployRequest { id: host_id, image_id }).await {
                                                    Ok(_) => on_deployed.call(()),
                                                    Err(err) => toast_error("Deploy failed", err.to_string()),
                                                }
                                            });
                                        },
                                        Icon { name: "deploy", class: "w-3.5 h-3.5" }
                                        "Deploy"
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => rsx! {
                        EmptyState { title: "Failed to load images", hint: e.to_string() }
                    },
                    None => rsx! {
                        div { class: "flex justify-center py-8", Spinner {} }
                    },
                }
            }
        }
    }
}
