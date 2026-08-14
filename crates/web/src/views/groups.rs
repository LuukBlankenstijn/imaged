use std::collections::HashSet;

use dioxus::prelude::*;

use crate::api::groups::{
    create_group, delete_group, get_all_groups, multicast, update_group_memberships,
    update_group_name,
};
use crate::api::hosts::{get_all_hosts, reboot, wake_on_lan};
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
use crate::model::{
    CreateGroupRequest, Group, Host, Image, ImageStatus, MulticastRequest, UpdateGroupRequest,
    UpdateName,
};

#[component]
pub fn Groups() -> Element {
    let mut groups = use_poll(5000, || async move { get_all_groups().await });
    let all_hosts = use_resource(|| async move { get_all_hosts(None).await });
    let open = use_signal(|| None::<i64>);

    let mut name = use_signal(String::new);
    let selected = use_signal(HashSet::<i64>::new);

    let create = move |_| {
        let group_name = name().trim().to_string();
        let host_ids: Vec<i64> = selected().iter().copied().collect();
        if group_name.is_empty() {
            return;
        }
        let mut name = name;
        let mut selected = selected;
        spawn(async move {
            match create_group(CreateGroupRequest {
                name: group_name,
                host_ids,
            })
            .await
            {
                Ok(_) => {
                    name.set(String::new());
                    selected.write().clear();
                    groups.restart();
                }
                Err(err) => toast_error("Create group failed", err.to_string()),
            }
        });
    };

    rsx! {
        PageHeader { title: "Groups", subtitle: "Named sets of hosts for batch operations" }
        Card { class: "mb-6 p-4",
            div { class: "mb-3 font-display text-sm uppercase tracking-wider text-fog-400", "New group" }
            input {
                class: "mb-3 w-full max-w-sm rounded-md border border-ink-600 bg-ink-900 px-3 py-2 text-sm text-fog-100 focus:outline-none focus-visible:glow-amber",
                placeholder: "Group name",
                value: "{name}",
                oninput: move |e| name.set(e.value()),
            }
            match &*all_hosts.read() {
                Some(Ok(list)) => rsx! {
                    HostPicker { hosts: list.clone(), selected }
                },
                Some(Err(e)) => rsx! {
                    p { class: "text-sm text-bad", "Failed to load hosts: {e}" }
                },
                None => rsx! {
                    div { class: "py-4", Spinner {} }
                },
            }
            div { class: "mt-4",
                Button { onclick: create,
                    Icon { name: "plus", class: "w-3.5 h-3.5" }
                    "Create group"
                }
            }
        }
        match &*groups.read() {
            Some(Ok(list)) if list.is_empty() => rsx! {
                Card { EmptyState { title: "No groups yet", hint: "Create one above to batch reboots, wakes and multicasts." } }
            },
            Some(Ok(list)) => rsx! {
                div { class: "flex flex-col gap-2",
                    for (i , g) in list.iter().enumerate() {
                        GroupRow {
                            key: "{g.id}",
                            group: g.clone(),
                            index: i,
                            open,
                            on_changed: move |_| groups.restart(),
                        }
                    }
                }
            },
            Some(Err(e)) => rsx! {
                Card { EmptyState { title: "Failed to load groups", hint: e.to_string() } }
            },
            None => rsx! {
                div { class: "flex justify-center py-16", Spinner {} }
            },
        }
    }
}

#[component]
fn HostPicker(hosts: Vec<Host>, selected: Signal<HashSet<i64>>) -> Element {
    let mut search = use_signal(String::new);
    let query = search().to_lowercase();
    let filtered: Vec<Host> = hosts
        .iter()
        .filter(|h| {
            query.is_empty()
                || h.name.to_lowercase().contains(&query)
                || h.mac_address.to_lowercase().contains(&query)
        })
        .cloned()
        .collect();
    let count = selected().len();

    rsx! {
        div { class: "max-w-sm",
            div { class: "relative mb-2",
                span { class: "pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-fog-600",
                    Icon { name: "search", class: "w-4 h-4" }
                }
                input {
                    class: "w-full rounded-md border border-ink-600 bg-ink-900 py-1.5 pl-8 pr-3 text-sm text-fog-100 focus:outline-none focus-visible:glow-amber",
                    placeholder: "Search hosts",
                    value: "{search}",
                    oninput: move |e| search.set(e.value()),
                }
            }
            div { class: "flex max-h-48 flex-wrap gap-1.5 overflow-y-auto rounded-md border border-line bg-ink-900/60 p-2",
                if filtered.is_empty() {
                    span { class: "px-1 py-2 text-xs text-fog-600", "No hosts" }
                }
                for h in filtered.iter() {
                    {
                        let id = h.id;
                        let active = selected().contains(&id);
                        let chip = if active {
                            "border-amber-500/50 bg-amber-500/15 text-amber-300"
                        } else {
                            "border-ink-600 text-fog-400 hover:border-ink-500 hover:text-fog-200"
                        };
                        rsx! {
                            button {
                                key: "{id}",
                                class: "rounded-full border px-2.5 py-1 text-xs transition-colors {chip}",
                                onclick: move |_| {
                                    let mut selected = selected;
                                    if selected.read().contains(&id) {
                                        selected.write().remove(&id);
                                    } else {
                                        selected.write().insert(id);
                                    }
                                },
                                "{h.name}"
                            }
                        }
                    }
                }
            }
            div { class: "mt-1.5 font-mono text-[11px] text-fog-600", "{count} selected" }
        }
    }
}

#[component]
fn GroupRow(
    group: Group,
    index: usize,
    open: Signal<Option<i64>>,
    on_changed: EventHandler<()>,
) -> Element {
    let group_id = group.id;
    let name = group.name.clone();
    let delay = index * 40;
    let expanded = open() == Some(group_id);

    let mut editing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut members_open = use_signal(|| false);
    let mut multicast_open = use_signal(|| false);
    let mut reboot_open = use_signal(|| false);
    let mut delete_open = use_signal(|| false);

    let menu_name = name.clone();
    let reboot_name = name.clone();

    let toggle = move |_| {
        let mut open = open;
        if open() == Some(group_id) {
            open.set(None);
        } else {
            open.set(Some(group_id));
        }
    };

    rsx! {
        Card { class: "rise overflow-hidden", style: "--d: {delay}ms",
            div { class: "flex items-center gap-3 px-4 py-3",
                button {
                    class: "flex min-w-0 flex-1 items-center gap-3 text-left",
                    onclick: toggle,
                    Icon {
                        name: "chevron",
                        class: if expanded { "w-4 h-4 rotate-180 text-fog-500 transition-transform" } else { "w-4 h-4 text-fog-500 transition-transform" },
                    }
                    Icon { name: "group", class: "w-4 h-4 text-amber-500" }
                    if editing() {
                        input {
                            class: "w-56 rounded-md border border-ink-600 bg-ink-900 px-2 py-1 text-sm text-fog-100 focus:outline-none focus-visible:glow-amber",
                            value: "{draft}",
                            autofocus: true,
                            onclick: move |e: MouseEvent| e.stop_propagation(),
                            oninput: move |e| draft.set(e.value()),
                            onkeydown: move |e| {
                                match e.key() {
                                    Key::Enter => {
                                        let new_name = draft().trim().to_string();
                                        editing.set(false);
                                        if !new_name.is_empty() {
                                            spawn(async move {
                                                match update_group_name(UpdateName { id: group_id, new_name }).await {
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
                    } else {
                        span { class: "truncate font-medium text-fog-100", "{name}" }
                    }
                }
                ActionMenu {
                    MenuItem {
                        label: "Rename",
                        icon: "edit",
                        onclick: move |_| {
                            draft.set(menu_name.clone());
                            editing.set(true);
                        },
                    }
                    MenuItem {
                        label: "Edit members",
                        icon: "group",
                        onclick: move |_| members_open.set(true),
                    }
                    MenuItem {
                        label: "Multicast",
                        icon: "multicast",
                        onclick: move |_| multicast_open.set(true),
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
                            spawn(async move {
                                match group_host_ids(group_id).await {
                                    Ok(ids) if !ids.is_empty() => {
                                        match wake_on_lan(ids).await {
                                            Ok(_) => toast_success("Wake sent", "Group members woken"),
                                            Err(err) => toast_error("Wake failed", err.to_string()),
                                        }
                                    }
                                    Ok(_) => toast_error("Wake failed", "Group has no members"),
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
            }
            if expanded {
                MemberList { group_id }
            }
            if members_open() {
                EditMembersModal {
                    group_id,
                    onclose: move |_| members_open.set(false),
                    on_saved: move |_| {
                        members_open.set(false);
                        on_changed.call(());
                    },
                }
            }
            if multicast_open() {
                MulticastModal {
                    group_id,
                    onclose: move |_| multicast_open.set(false),
                    on_done: move |_| {
                        multicast_open.set(false);
                        on_changed.call(());
                    },
                }
            }
            ConfirmDialog {
                open: reboot_open(),
                title: "Reboot group",
                message: format!("Reboot every host in {reboot_name}?"),
                confirm_label: "Reboot",
                onconfirm: move |_| {
                    reboot_open.set(false);
                    spawn(async move {
                        match group_host_ids(group_id).await {
                            Ok(ids) if !ids.is_empty() => {
                                match reboot(ids).await {
                                    Ok(_) => on_changed.call(()),
                                    Err(err) => toast_error("Reboot failed", err.to_string()),
                                }
                            }
                            Ok(_) => toast_error("Reboot failed", "Group has no members"),
                            Err(err) => toast_error("Reboot failed", err.to_string()),
                        }
                    });
                },
                oncancel: move |_| reboot_open.set(false),
            }
            ConfirmDialog {
                open: delete_open(),
                title: "Delete group",
                message: format!("Delete {name}? Member hosts are not affected."),
                confirm_label: "Delete",
                danger: true,
                onconfirm: move |_| {
                    delete_open.set(false);
                    spawn(async move {
                        match delete_group(group_id).await {
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

async fn group_host_ids(group_id: i64) -> Result<Vec<i64>, ServerFnError> {
    let hosts = get_all_hosts(Some(group_id)).await?;
    Ok(hosts.iter().map(|h| h.id).collect())
}

#[component]
fn MemberList(group_id: i64) -> Element {
    let members = use_resource(move || async move { get_all_hosts(Some(group_id)).await });
    rsx! {
        div { class: "border-t border-line/60 px-4 py-3",
            match &*members.read() {
                Some(Ok(list)) if list.is_empty() => rsx! {
                    p { class: "text-sm text-fog-600", "No members." }
                },
                Some(Ok(list)) => rsx! {
                    div { class: "flex flex-col gap-1.5",
                        for h in list.iter() {
                            MemberRow { key: "{h.id}", host: h.clone() }
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    p { class: "text-sm text-bad", "Failed to load members: {e}" }
                },
                None => rsx! {
                    Spinner {}
                },
            }
        }
    }
}

#[component]
fn MemberRow(host: Host) -> Element {
    let connected = use_connection(host.id);
    rsx! {
        div { class: "flex items-center gap-3 text-sm",
            StatusDot { connected }
            span { class: "text-fog-200", "{host.name}" }
            span { class: "font-mono text-xs text-fog-500", "{host.mac_address}" }
        }
    }
}

#[component]
fn EditMembersModal(
    group_id: i64,
    onclose: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let all_hosts = use_resource(|| async move { get_all_hosts(None).await });
    let members = use_resource(move || async move { get_all_hosts(Some(group_id)).await });
    let selected = use_signal(HashSet::<i64>::new);
    let mut seeded = use_signal(|| false);

    use_effect(move || {
        if !seeded() {
            if let Some(Ok(list)) = &*members.read() {
                let mut selected = selected;
                selected.set(list.iter().map(|h| h.id).collect());
                seeded.set(true);
            }
        }
    });

    rsx! {
        Modal { open: true, onclose: move |_| onclose.call(()), title: "Edit members",
            match &*all_hosts.read() {
                Some(Ok(list)) => rsx! {
                    HostPicker { hosts: list.clone(), selected }
                    div { class: "mt-6 flex justify-end gap-2",
                        Button { variant: ButtonVariant::Ghost, onclick: move |_| onclose.call(()), "Cancel" }
                        Button {
                            onclick: move |_| {
                                let host_ids: Vec<i64> = selected().iter().copied().collect();
                                spawn(async move {
                                    match update_group_memberships(UpdateGroupRequest { id: group_id, host_ids }).await {
                                        Ok(_) => on_saved.call(()),
                                        Err(err) => toast_error("Update failed", err.to_string()),
                                    }
                                });
                            },
                            "Save"
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    EmptyState { title: "Failed to load hosts", hint: e.to_string() }
                },
                None => rsx! {
                    div { class: "flex justify-center py-8", Spinner {} }
                },
            }
        }
    }
}

#[component]
fn MulticastModal(group_id: i64, onclose: EventHandler<()>, on_done: EventHandler<()>) -> Element {
    let images = use_resource(|| async move { get_all_images().await });
    let mut selected = use_signal(|| None::<i64>);

    rsx! {
        Modal { open: true, onclose: move |_| onclose.call(()), title: "Multicast image",
            p { class: "mb-4 text-sm text-fog-400", "Stream a ready image to every host in this group simultaneously." }
            match &*images.read() {
                Some(Ok(list)) => {
                    let ready: Vec<Image> = list
                        .iter()
                        .filter(|i| i.status == ImageStatus::Ready)
                        .cloned()
                        .collect();
                    if ready.is_empty() {
                        rsx! {
                            EmptyState { title: "No ready images", hint: "Capture an image first." }
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
                                Button { variant: ButtonVariant::Ghost, onclick: move |_| onclose.call(()), "Cancel" }
                                Button {
                                    onclick: move |_| {
                                        let image_id = selected().or(default_id);
                                        spawn(async move {
                                            let Some(image_id) = image_id else { return };
                                            match group_host_ids(group_id).await {
                                                Ok(ids) if !ids.is_empty() => {
                                                    match multicast(MulticastRequest { host_ids: ids, image_id }).await {
                                                        Ok(_) => on_done.call(()),
                                                        Err(err) => toast_error("Multicast failed", err.to_string()),
                                                    }
                                                }
                                                Ok(_) => toast_error("Multicast failed", "Group has no members"),
                                                Err(err) => toast_error("Multicast failed", err.to_string()),
                                            }
                                        });
                                    },
                                    Icon { name: "multicast", class: "w-3.5 h-3.5" }
                                    "Multicast"
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
