use std::collections::BTreeMap;

use dioxus::prelude::*;

use crate::components::icons::Icon;
use crate::components::ui::FIELD_CLASSES;
use crate::format::format_bytes;
use crate::model::{Host, HostPartition, Image, ImageKind};

pub(crate) fn partition_label(partition: &HostPartition) -> Option<&str> {
    [partition.partlabel.as_deref(), partition.label.as_deref()]
        .into_iter()
        .flatten()
        .find(|label| !label.is_empty())
}

pub(crate) fn partition_name(partition: &HostPartition) -> String {
    match partition_label(partition) {
        Some(label) => format!("#{} ({label})", partition.number),
        None => format!("#{}", partition.number),
    }
}

pub(crate) fn partition_summary(partition: &HostPartition) -> String {
    let fstype = partition.fstype.as_deref().unwrap_or("no filesystem");
    format!(
        "{} · {fstype} · {}",
        partition_name(partition),
        format_bytes(partition.size_bytes)
    )
}

pub(crate) fn capture_sources(host: &Host) -> Vec<HostPartition> {
    host.partitions
        .iter()
        .filter(|p| {
            p.fstype
                .as_deref()
                .is_some_and(|fstype| ImageKind::Partition.fstypes().contains(&fstype))
        })
        .cloned()
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unfit {
    TooSmall { needed: u64 },
    HoldsFilesystem { twin: String },
    HoldsLabel { twin: String, label: String },
}

impl Unfit {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::TooSmall { needed } => format!("too small, needs {}", format_bytes(*needed)),
            Self::HoldsFilesystem { twin } => {
                format!("{twin} already holds this image's filesystem")
            }
            Self::HoldsLabel { twin, label } => {
                format!("{twin} already holds a filesystem labelled `{label}`")
            }
        }
    }
}

pub(crate) fn unfit(image: &Image, host: &Host, target: &HostPartition) -> Option<Unfit> {
    let source = image.partitions.first()?;
    if target.size_bytes < source.size_bytes {
        return Some(Unfit::TooSmall {
            needed: source.size_bytes,
        });
    }
    let twin = |holds: &dyn Fn(&HostPartition) -> bool| {
        host.partitions
            .iter()
            .find(|other| other.number != target.number && holds(other))
    };
    if let Some(fs_uuid) = source.fs_uuid.as_deref()
        && let Some(twin) = twin(&|other| {
            other
                .uuid
                .as_deref()
                .is_some_and(|uuid| uuid.eq_ignore_ascii_case(fs_uuid))
        })
    {
        return Some(Unfit::HoldsFilesystem {
            twin: partition_name(twin),
        });
    }
    let fs_label = source.fs_label.as_ref()?;
    twin(&|other| other.label.as_ref() == Some(fs_label)).map(|twin| Unfit::HoldsLabel {
        twin: partition_name(twin),
        label: fs_label.clone(),
    })
}

pub(crate) fn deploy_target(image: &Image, host: &Host, partuuid: &str) -> Option<HostPartition> {
    host.partitions
        .iter()
        .find(|p| p.partuuid == partuuid && unfit(image, host, p).is_none())
        .cloned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MatchBy {
    Label,
    Number,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    Target(HostPartition),
    NoMatch,
    Ambiguous(usize),
    Unfit(HostPartition, Unfit),
}

pub(crate) fn match_partitions<'a>(
    host: &'a Host,
    by: MatchBy,
    value: &str,
) -> Vec<&'a HostPartition> {
    let value = value.trim();
    match by {
        MatchBy::Label if !value.is_empty() => host
            .partitions
            .iter()
            .filter(|p| p.partlabel.as_deref() == Some(value))
            .collect(),
        MatchBy::Number => match value.parse::<i64>() {
            Ok(number) => host
                .partitions
                .iter()
                .filter(|p| p.number == number)
                .collect(),
            Err(_) => Vec::new(),
        },
        MatchBy::Label => Vec::new(),
    }
}

pub(crate) fn resolve(image: &Image, host: &Host, by: MatchBy, value: &str) -> Resolution {
    match match_partitions(host, by, value).as_slice() {
        [] => Resolution::NoMatch,
        [target] => match unfit(image, host, target) {
            Some(reason) => Resolution::Unfit((*target).clone(), reason),
            None => Resolution::Target((*target).clone()),
        },
        many => Resolution::Ambiguous(many.len()),
    }
}

pub(crate) fn multicast_targets(
    image: &Image,
    hosts: &[Host],
    by: MatchBy,
    value: &str,
) -> Option<BTreeMap<i64, String>> {
    hosts
        .iter()
        .map(|host| match resolve(image, host, by, value) {
            Resolution::Target(target) => Some((host.id, target.partuuid)),
            _ => None,
        })
        .collect()
}

#[component]
pub fn HostPartitionList(partitions: Vec<HostPartition>) -> Element {
    rsx! {
        div { class: "overflow-x-auto rounded-md border border-line bg-ink-900/60",
            table { class: "w-full font-mono text-xs",
                thead {
                    tr { class: "text-fog-500",
                        th { class: "px-3 py-1.5 text-left text-[11px] font-medium uppercase tracking-wider", "#" }
                        th { class: "px-3 py-1.5 text-left text-[11px] font-medium uppercase tracking-wider", "Partlabel" }
                        th { class: "px-3 py-1.5 text-left text-[11px] font-medium uppercase tracking-wider", "Label" }
                        th { class: "px-3 py-1.5 text-left text-[11px] font-medium uppercase tracking-wider", "Filesystem" }
                        th { class: "px-3 py-1.5 text-right text-[11px] font-medium uppercase tracking-wider", "Size" }
                        th { class: "px-3 py-1.5 text-left text-[11px] font-medium uppercase tracking-wider", "Partuuid" }
                    }
                }
                tbody {
                    for p in partitions.iter() {
                        tr { key: "{p.partuuid}", class: "border-t border-line/60 text-fog-300",
                            td { class: "px-3 py-1.5 text-fog-500", "{p.number}" }
                            td { class: "px-3 py-1.5", {p.partlabel.clone().unwrap_or_else(|| "\u{2014}".to_string())} }
                            td { class: "px-3 py-1.5", {p.label.clone().unwrap_or_else(|| "\u{2014}".to_string())} }
                            td { class: "px-3 py-1.5", {p.fstype.clone().unwrap_or_else(|| "\u{2014}".to_string())} }
                            td { class: "px-3 py-1.5 text-right", {format_bytes(p.size_bytes)} }
                            td { class: "px-3 py-1.5 text-fog-600", "{p.partuuid}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn OverwriteWarning(message: String) -> Element {
    rsx! {
        div {
            role: "alert",
            class: "mt-4 flex items-start gap-2 rounded-md border border-bad/30 bg-bad/10 px-3 py-2 text-sm text-bad",
            Icon { name: "alert", class: "mt-0.5 w-4 h-4 shrink-0" }
            span { "{message}" }
        }
    }
}

pub(crate) fn overwrite_message(target: &HostPartition, host: &Host) -> String {
    format!(
        "Overwrites partition {} on {}; other partitions are untouched. This cannot be undone.",
        partition_name(target),
        host.name
    )
}

#[component]
pub fn PartitionTargetPicker(
    image: Image,
    host: Host,
    selected: Signal<Option<String>>,
) -> Element {
    let mut selected = selected;
    let current = selected().filter(|uuid| deploy_target(&image, &host, uuid).is_some());
    let needed = image.partitions.first().map(|p| format_bytes(p.size_bytes));

    rsx! {
        div { class: "mt-4",
            label {
                class: "mb-1 block font-mono text-[11px] uppercase tracking-wider text-fog-500",
                r#for: "deploy-partition",
                "Target partition"
            }
            if host.partitions.is_empty() {
                p { class: "rounded-md border border-line bg-ink-900/60 px-3 py-2 text-xs text-fog-600",
                    "{host.name} has not reported any partitions. Boot it into the agent so it can report its disk layout."
                }
            } else {
                select {
                    id: "deploy-partition",
                    class: "{FIELD_CLASSES} w-full font-mono",
                    onchange: move |e| {
                        let value = e.value();
                        selected.set((!value.is_empty()).then_some(value));
                    },
                    option { value: "", disabled: true, selected: current.is_none(), "Choose the partition to overwrite\u{2026}" }
                    for p in host.partitions.iter() {
                        {
                            let problem = unfit(&image, &host, p);
                            let text = match &problem {
                                Some(reason) => format!("{} · {}", partition_summary(p), reason.reason()),
                                None => partition_summary(p),
                            };
                            rsx! {
                                option {
                                    key: "{p.partuuid}",
                                    value: "{p.partuuid}",
                                    disabled: problem.is_some(),
                                    selected: current.as_deref() == Some(p.partuuid.as_str()),
                                    "{text}"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(needed) = needed {
                p { class: "mt-1.5 text-xs text-fog-600",
                    "The image needs at least {needed}. A larger partition keeps its size and an ext filesystem is grown to fill it."
                }
            }
        }
    }
}

pub(crate) fn resolution_text(resolution: &Resolution) -> String {
    match resolution {
        Resolution::Target(target) => partition_summary(target),
        Resolution::NoMatch => "no match".to_string(),
        Resolution::Ambiguous(count) => format!("{count} partitions match"),
        Resolution::Unfit(target, reason) => {
            format!("{}: {}", partition_name(target), reason.reason())
        }
    }
}

#[component]
pub fn PartitionMatcher(
    image: Image,
    hosts: Vec<Host>,
    by: Signal<MatchBy>,
    value: Signal<String>,
) -> Element {
    let mut by = by;
    let mut value = value;
    let mode = by();
    let needle = value();
    let mut labels: Vec<String> = hosts
        .iter()
        .flat_map(|h| h.partitions.iter().filter_map(|p| p.partlabel.clone()))
        .filter(|label| !label.is_empty())
        .collect();
    labels.sort();
    labels.dedup();
    let resolved = hosts
        .iter()
        .filter(|h| matches!(resolve(&image, h, mode, &needle), Resolution::Target(_)))
        .count();

    rsx! {
        div { class: "mt-4",
            div { class: "mb-1 font-mono text-[11px] uppercase tracking-wider text-fog-500",
                "Target partition on each host"
            }
            div { class: "flex flex-wrap items-center gap-2",
                select {
                    class: "{FIELD_CLASSES} font-mono",
                    aria_label: "Match partitions by",
                    onchange: move |e| {
                        by.set(if e.value() == "number" { MatchBy::Number } else { MatchBy::Label });
                        value.set(String::new());
                    },
                    option { value: "label", selected: mode == MatchBy::Label, "PARTLABEL" }
                    option { value: "number", selected: mode == MatchBy::Number, "number" }
                }
                match mode {
                    MatchBy::Label => rsx! {
                        input {
                            class: "{FIELD_CLASSES} min-w-48 flex-1 font-mono",
                            aria_label: "Partition label",
                            placeholder: "root",
                            list: "multicast-partlabels",
                            value: "{needle}",
                            oninput: move |e| value.set(e.value()),
                        }
                        datalist { id: "multicast-partlabels",
                            for label in labels.iter() {
                                option { key: "{label}", value: "{label}" }
                            }
                        }
                    },
                    MatchBy::Number => rsx! {
                        input {
                            class: "{FIELD_CLASSES} w-28 font-mono",
                            aria_label: "Partition number",
                            r#type: "number",
                            min: "1",
                            placeholder: "2",
                            value: "{needle}",
                            oninput: move |e| value.set(e.value()),
                        }
                    },
                }
                span { class: "font-mono text-xs text-fog-500", "{resolved}/{hosts.len()} resolved" }
            }
            ul { class: "mt-2 max-h-56 overflow-y-auto rounded-md border border-line font-mono text-xs",
                for host in hosts.iter() {
                    {
                        let resolution = resolve(&image, host, mode, &needle);
                        let (tone, text) = match &resolution {
                            _ if needle.trim().is_empty() => ("text-fog-600", "\u{2014}".to_string()),
                            Resolution::Target(_) => ("text-fog-300", resolution_text(&resolution)),
                            _ => ("text-bad", resolution_text(&resolution)),
                        };
                        rsx! {
                            li { key: "{host.id}", class: "flex items-center gap-3 border-t border-line/60 px-3 py-1.5 first:border-t-0",
                                span { class: "w-40 shrink-0 truncate text-fog-100", "{host.name}" }
                                span { class: "min-w-0 flex-1 truncate {tone}", "{text}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
