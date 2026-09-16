use dioxus::prelude::*;

use crate::api::hosts::{
    delete_host, deploy, get_all_hosts, reboot, update_host_name, wake_on_lan,
};
use crate::api::images::get_all_images;
use crate::components::connection::{Connections, use_connection};
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
    let conns = use_context::<Connections>().0;
    let mut query = use_signal(String::new);
    let mut show_online = use_signal(|| false);
    let mut show_offline = use_signal(|| false);

    rsx! {
        PageHeader { title: "Hosts", subtitle: "Registered machines on the netboot fabric" }
        Card { class: "overflow-hidden",
            {
                match &*hosts.read() {
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        EmptyState {
                            title: "No hosts detected",
                            hint: "Machines appear here once they PXE-boot and register.",
                        }
                    },
                    Some(Ok(list)) => {
                        let total = list.len();
                        let needle = query().trim().to_lowercase();
                        let want_online = show_online();
                        let want_offline = show_offline();
                        let (online, visible) = {
                            let map = conns.read();
                            let connected = |id: i64| map.get(&id).copied().unwrap_or(false);
                            let online = list.iter().filter(|h| connected(h.id)).count();
                            let visible = visible_hosts(
                                list,
                                &needle,
                                want_online,
                                want_offline,
                                &connected,
                            );
                            (online, visible)
                        };
                        rsx! {
                            div { class: "flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3",
                                div { class: "relative min-w-0 max-w-sm flex-1",
                                    span { class: "pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-fog-600",
                                        Icon { name: "search", class: "w-4 h-4" }
                                    }
                                    input {
                                        class: "w-full rounded-md border border-ink-600 bg-ink-900 py-1.5 pl-8 pr-3 text-sm text-fog-100 focus:outline-none focus-visible:glow-amber",
                                        placeholder: "Search name, MAC or IP",
                                        value: "{query}",
                                        oninput: move |e| query.set(e.value()),
                                    }
                                }
                                div { class: "flex shrink-0 items-center gap-3",
                                    div { class: "flex items-center gap-1.5",
                                        span { class: "font-mono text-[11px] uppercase tracking-wider text-fog-600",
                                            "Connection"
                                        }
                                        button {
                                            class: "rounded-full border px-2.5 py-1 text-xs transition-colors {chip_classes(want_online)}",
                                            onclick: move |_| show_online.toggle(),
                                            "Online"
                                        }
                                        button {
                                            class: "rounded-full border px-2.5 py-1 text-xs transition-colors {chip_classes(want_offline)}",
                                            onclick: move |_| show_offline.toggle(),
                                            "Offline"
                                        }
                                    }
                                    div { class: "ml-2 flex items-center gap-1.5",
                                        StatusDot { connected: online > 0 }
                                        span { class: "font-mono text-xs text-fog-300", "{online} / {total}" }
                                        span { class: "text-[11px] uppercase tracking-wider text-fog-500",
                                            "online"
                                        }
                                    }
                                }
                            }
                            if visible.is_empty() {
                                EmptyState {
                                    title: "No matching hosts",
                                    hint: "Adjust the search term or the connection filter.",
                                }
                            } else {
                                div { class: "overflow-x-auto",
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
                                            for (i , h) in visible.iter().enumerate() {
                                                HostRow {
                                                    key: "{h.id}",
                                                    host: h.clone(),
                                                    index: i,
                                                    on_changed: move |_| hosts.restart(),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
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

fn chip_classes(active: bool) -> &'static str {
    if active {
        "border-amber-500/50 bg-amber-500/15 text-amber-300"
    } else {
        "border-ink-600 text-fog-400 hover:border-ink-500 hover:text-fog-100"
    }
}

pub(crate) fn connection_allows(want_online: bool, want_offline: bool, connected: bool) -> bool {
    want_online == want_offline || want_online == connected
}

pub(crate) fn visible_hosts(
    hosts: &[Host],
    needle: &str,
    want_online: bool,
    want_offline: bool,
    connected: impl Fn(i64) -> bool,
) -> Vec<Host> {
    hosts
        .iter()
        .filter(|h| connection_allows(want_online, want_offline, connected(h.id)))
        .filter(|h| host_matches(h, needle))
        .cloned()
        .collect()
}

pub(crate) fn host_matches(host: &Host, needle: &str) -> bool {
    needle.is_empty()
        || host.name.to_lowercase().contains(needle)
        || host.mac_address.to_lowercase().contains(needle)
        || matches!(&host.ip, Some(ip) if ip.to_lowercase().contains(needle))
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

#[cfg(test)]
mod tests {
    use super::{connection_allows, host_matches, visible_hosts};
    use crate::model::Host;

    fn host(id: i64, name: &str, mac: &str, ip: Option<&str>) -> Host {
        Host {
            id,
            mac_address: mac.to_string(),
            name: name.to_string(),
            disk_size_bytes: 0,
            ip: ip.map(str::to_string),
        }
    }

    fn fleet() -> Vec<Host> {
        vec![
            host(1, "alpha", "AA:BB:CC:00:00:01", Some("192.168.1.10")),
            host(2, "beta", "11:22:33:00:00:02", Some("10.0.0.7")),
            host(3, "gamma", "aa:bb:cc:00:00:03", None),
        ]
    }

    fn names(hosts: &[Host]) -> Vec<&str> {
        hosts.iter().map(|h| h.name.as_str()).collect()
    }

    #[test]
    fn host_matches_is_case_insensitive_across_name_mac_and_ip() {
        let hosts = fleet();
        assert!(host_matches(&hosts[0], "lph"));
        assert!(host_matches(&hosts[0], "aa:bb"));
        assert!(host_matches(&hosts[2], "aa:bb"));
        assert!(host_matches(&hosts[0], "192.168"));
        assert!(!host_matches(&hosts[1], "192.168"));
        assert!(!host_matches(&hosts[2], "192.168"));
        assert!(host_matches(&hosts[1], ""));
    }

    #[test]
    fn connection_filter_is_inert_when_both_toggles_agree() {
        for connected in [true, false] {
            assert!(connection_allows(false, false, connected));
            assert!(connection_allows(true, true, connected));
        }
        assert!(connection_allows(true, false, true));
        assert!(!connection_allows(true, false, false));
        assert!(connection_allows(false, true, false));
        assert!(!connection_allows(false, true, true));
    }

    #[test]
    fn visible_hosts_narrows_by_mac_ip_and_name() {
        let hosts = fleet();
        let all = |_: i64| true;
        assert_eq!(
            names(&visible_hosts(&hosts, "aa:bb", false, false, all)),
            ["alpha", "gamma"]
        );
        assert_eq!(
            names(&visible_hosts(&hosts, "192.168", false, false, all)),
            ["alpha"]
        );
        assert_eq!(
            names(&visible_hosts(&hosts, "amm", false, false, all)),
            ["gamma"]
        );
        assert_eq!(
            names(&visible_hosts(&hosts, "", false, false, all)),
            ["alpha", "beta", "gamma"]
        );
    }

    #[test]
    fn visible_hosts_composes_connection_toggles_with_the_search_term() {
        let hosts = fleet();
        let connected = |id: i64| id == 1 || id == 2;
        assert_eq!(
            names(&visible_hosts(&hosts, "", true, false, connected)),
            ["alpha", "beta"]
        );
        assert_eq!(
            names(&visible_hosts(&hosts, "", false, true, connected)),
            ["gamma"]
        );
        assert_eq!(
            names(&visible_hosts(&hosts, "", true, true, connected)),
            ["alpha", "beta", "gamma"]
        );
        assert_eq!(
            names(&visible_hosts(&hosts, "aa:bb", true, false, connected)),
            ["alpha"]
        );
        assert_eq!(
            names(&visible_hosts(&hosts, "aa:bb", false, true, connected)),
            ["gamma"]
        );
        assert!(visible_hosts(&hosts, "beta", false, true, connected).is_empty());
    }
}
