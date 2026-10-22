use dioxus::prelude::*;

use crate::api::scripts::{get_image_scripts, get_script_variable_names};
use crate::components::icons::Icon;
use crate::components::ui::{FIELD_CLASSES, Spinner};
use crate::format::format_bytes;
use crate::model::{
    DeployInputs, DeployRequest, Host, Image, InputKind, InputSource, InputSources,
    MulticastRequest, Script, ScriptInput,
};

pub(crate) fn form_fields(scripts: &[Script]) -> Vec<ScriptInput> {
    let mut fields: Vec<ScriptInput> = Vec::new();
    for input in scripts.iter().flat_map(|s| &s.inputs) {
        if !fields.iter().any(|f| f.name == input.name) {
            fields.push(input.clone());
        }
    }
    fields
}

pub(crate) fn initial_inputs(fields: &[ScriptInput], host_ids: &[i64]) -> DeployInputs {
    let defaults = |per_machine: bool| -> InputSources {
        fields
            .iter()
            .filter(|f| f.per_machine == per_machine)
            .filter_map(|f| Some((f.name.clone(), InputSource::Value(f.default_value.clone()?))))
            .collect()
    };
    let per_host = defaults(true);
    DeployInputs {
        shared: defaults(false),
        per_machine: host_ids.iter().map(|&id| (id, per_host.clone())).collect(),
    }
}

pub(crate) fn submitted_inputs(form: &DeployInputs) -> DeployInputs {
    let filled = |sources: &InputSources| -> InputSources {
        sources
            .iter()
            .filter(|(_, source)| **source != InputSource::Value(String::new()))
            .map(|(name, source)| (name.clone(), source.clone()))
            .collect()
    };
    DeployInputs {
        shared: filled(&form.shared),
        per_machine: form
            .per_machine
            .iter()
            .map(|(&host_id, sources)| (host_id, filled(sources)))
            .filter(|(_, sources)| !sources.is_empty())
            .collect(),
    }
}

pub(crate) fn deploy_request(
    host_id: i64,
    image_id: Option<i64>,
    form: Option<&DeployInputs>,
) -> Option<DeployRequest> {
    Some(DeployRequest {
        id: host_id,
        image_id: image_id?,
        inputs: submitted_inputs(form?),
    })
}

pub(crate) fn multicast_request(
    host_ids: &[i64],
    image_id: Option<i64>,
    form: Option<&DeployInputs>,
) -> Option<MulticastRequest> {
    if host_ids.is_empty() {
        return None;
    }
    Some(MulticastRequest {
        host_ids: host_ids.to_vec(),
        image_id: image_id?,
        inputs: submitted_inputs(form?),
    })
}

pub(crate) fn erase_target(host: &Host) -> String {
    let disk = host
        .disk_size_bytes
        .map(format_bytes)
        .unwrap_or_else(|| "no usable disk".to_string());
    format!("{} · {} · {disk}", host.name, host.mac_address)
}

#[component]
pub fn EraseWarning(hosts: Vec<Host>) -> Element {
    let target = match hosts.len() {
        1 => "this host".to_string(),
        n => format!("these {n} hosts"),
    };
    rsx! {
        div {
            role: "alert",
            class: "mb-4 rounded-md border border-bad/30 bg-bad/10 px-3 py-2 text-sm text-bad",
            div { class: "flex items-start gap-2",
                Icon { name: "alert", class: "mt-0.5 w-4 h-4 shrink-0" }
                span { "Erases every partition on the disk of {target} and replaces it with the image. This cannot be undone." }
            }
            ul { class: "mt-2 max-h-40 overflow-y-auto pl-6 font-mono text-xs",
                for host in hosts.iter() {
                    li { key: "{host.id}", {erase_target(host)} }
                }
            }
        }
    }
}

#[component]
pub fn ImagePicker(images: Vec<Image>, selected: Signal<Option<i64>>) -> Element {
    let mut selected = selected;
    let current = selected();
    rsx! {
        label {
            class: "mb-1 block font-mono text-[11px] uppercase tracking-wider text-fog-500",
            r#for: "deploy-image",
            "Image"
        }
        select {
            id: "deploy-image",
            class: "{FIELD_CLASSES} w-full",
            onchange: move |e| selected.set(e.value().parse::<i64>().ok()),
            option { value: "", disabled: true, selected: current.is_none(), "Choose an image\u{2026}" }
            for image in images.iter() {
                option {
                    key: "{image.id}",
                    value: "{image.id}",
                    selected: current == Some(image.id),
                    "{image.name}"
                }
            }
        }
    }
}

fn field_label(input: &ScriptInput) -> &str {
    if input.label.is_empty() {
        &input.name
    } else {
        &input.label
    }
}

#[component]
pub fn DeployInputsForm(
    image: ReadSignal<Image>,
    hosts: Vec<Host>,
    inputs: Signal<Option<DeployInputs>>,
) -> Element {
    let scripts = use_resource(move || {
        let image_id = image.read().id;
        async move { get_image_scripts(image_id).await }
    });
    let variables = use_resource(get_script_variable_names);
    let host_ids: Vec<i64> = hosts.iter().map(|h| h.id).collect();

    use_effect(move || {
        let mut inputs = inputs;
        let ready = scripts.state()() == UseResourceState::Ready;
        match &*scripts.read() {
            Some(Ok(list)) if ready => {
                inputs.set(Some(initial_inputs(&form_fields(list), &host_ids)))
            }
            _ => inputs.set(None),
        }
    });

    let ready = scripts.state()() == UseResourceState::Ready;
    match &*scripts.read() {
        Some(Ok(list)) if ready && list.is_empty() => rsx! {},
        Some(Ok(list)) if ready => {
            let fields = form_fields(list);
            let shared: Vec<ScriptInput> =
                fields.iter().filter(|f| !f.per_machine).cloned().collect();
            let per_machine: Vec<ScriptInput> =
                fields.iter().filter(|f| f.per_machine).cloned().collect();
            let names: Vec<String> = list.iter().map(|s| s.name.clone()).collect();
            let values = inputs().unwrap_or_default();
            let variables: Vec<String> = match &*variables.read() {
                Some(Ok(names)) => names.clone(),
                _ => Vec::new(),
            };
            rsx! {
                div { class: "mt-5 flex flex-col gap-4 border-t border-line pt-4",
                    div { class: "flex flex-wrap items-center gap-1.5 text-xs text-fog-500",
                        Icon { name: "script", class: "w-3.5 h-3.5 text-amber-500" }
                        span { "Post-install scripts:" }
                        span { class: "font-mono text-fog-300", {names.join(" → ")} }
                    }
                    if image.read().root_partition.is_none() {
                        div { class: "flex items-start gap-2 rounded-md border border-warn/30 bg-warn/10 px-3 py-2 text-xs text-warn",
                            Icon { name: "alert", class: "mt-0.5 w-3.5 h-3.5 shrink-0" }
                            span { "This image has no root partition set, so the deploy will be refused. Pick one on the Images page." }
                        }
                    }
                    if !shared.is_empty() {
                        div { class: "grid gap-3 sm:grid-cols-2",
                            for field in shared.iter() {
                                div { key: "{field.name}",
                                    FieldLabel { input: field.clone(), target: format!("input-{}", field.name) }
                                    FieldControl {
                                        id: format!("input-{}", field.name),
                                        input: field.clone(),
                                        variables: variables.clone(),
                                        source: values.shared.get(&field.name).cloned(),
                                        onchange: {
                                            let name = field.name.clone();
                                            move |source: InputSource| {
                                                let mut inputs = inputs;
                                                if let Some(form) = inputs.write().as_mut() {
                                                    form.shared.insert(name.clone(), source);
                                                }
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    }
                    if !per_machine.is_empty() {
                        div {
                            div { class: "mb-1 font-mono text-[11px] uppercase tracking-wider text-fog-500",
                                "Per machine"
                            }
                            div { class: "max-h-72 overflow-auto rounded-md border border-line",
                                table { class: "w-full text-sm",
                                    thead {
                                        tr { class: "text-fog-500",
                                            th { class: "px-3 py-2 text-left text-[11px] font-medium uppercase tracking-wider", "Host" }
                                            for field in per_machine.iter() {
                                                th { key: "{field.name}", class: "px-3 py-2 text-left",
                                                    FieldLabel { input: field.clone() }
                                                }
                                            }
                                        }
                                    }
                                    tbody {
                                        for host in hosts.iter() {
                                            tr { key: "{host.id}", class: "border-t border-line/60",
                                                td { class: "px-3 py-1.5",
                                                    div { class: "text-fog-100", "{host.name}" }
                                                    div { class: "font-mono text-[11px] text-fog-600", "{host.mac_address}" }
                                                }
                                                for field in per_machine.iter() {
                                                    td { key: "{field.name}", class: "px-3 py-1.5",
                                                        FieldControl {
                                                            id: format!("input-{}-{}", host.id, field.name),
                                                            label: format!("{} for {}", field_label(field), host.name),
                                                            input: field.clone(),
                                                            variables: variables.clone(),
                                                            source: values
                                                                .per_machine
                                                                .get(&host.id)
                                                                .and_then(|v| v.get(&field.name))
                                                                .cloned(),
                                                            onchange: {
                                                                let name = field.name.clone();
                                                                let host_id = host.id;
                                                                move |source: InputSource| {
                                                                    let mut inputs = inputs;
                                                                    if let Some(form) = inputs.write().as_mut() {
                                                                        form.per_machine
                                                                            .entry(host_id)
                                                                            .or_default()
                                                                            .insert(name.clone(), source);
                                                                    }
                                                                }
                                                            },
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(e)) if ready => rsx! {
            p { class: "mt-4 text-sm text-bad", "Failed to load the image's scripts: {e}" }
        },
        _ => rsx! {
            div { class: "mt-4 flex justify-center", Spinner { class: "w-4 h-4" } }
        },
    }
}

#[component]
fn FieldLabel(input: ScriptInput, #[props(default)] target: String) -> Element {
    let label = field_label(&input).to_string();
    rsx! {
        label {
            class: "mb-1 block font-mono text-[11px] uppercase tracking-wider text-fog-500",
            r#for: (!target.is_empty()).then_some(target),
            "{label}"
            if input.required {
                span { class: "ml-0.5 text-amber-500", "*" }
            }
        }
        if let Some(description) = &input.description {
            p { class: "mb-1 whitespace-pre-line text-xs font-normal text-fog-600",
                "{description}"
            }
        }
    }
}

#[component]
fn FieldControl(
    id: String,
    #[props(default)] label: String,
    input: ScriptInput,
    variables: Vec<String>,
    source: Option<InputSource>,
    onchange: EventHandler<InputSource>,
) -> Element {
    let (variable, value) = match source {
        Some(InputSource::Variable(name)) => (Some(name), String::new()),
        Some(InputSource::Value(value)) => (None, value),
        None => (None, String::new()),
    };
    let named = if label.is_empty() {
        field_label(&input).to_string()
    } else {
        label.clone()
    };
    rsx! {
        div { class: "flex items-start gap-2",
            if !variables.is_empty() {
                select {
                    class: "{FIELD_CLASSES} w-28 shrink-0 text-xs",
                    title: "Take the value from a script variable",
                    aria_label: "Source of {named}",
                    onchange: move |e| {
                        let picked = e.value();
                        onchange.call(if picked.is_empty() {
                            InputSource::Value(String::new())
                        } else {
                            InputSource::Variable(picked)
                        });
                    },
                    option { value: "", selected: variable.is_none(), "value" }
                    for name in variables.iter() {
                        option {
                            key: "{name}",
                            value: "{name}",
                            selected: variable.as_deref() == Some(name.as_str()),
                            "${name}"
                        }
                    }
                }
            }
            ValueControl {
                id,
                label,
                input,
                value,
                disabled: variable.is_some(),
                onchange: move |value: String| onchange.call(InputSource::Value(value)),
            }
        }
    }
}

#[component]
fn ValueControl(
    id: String,
    label: String,
    input: ScriptInput,
    value: String,
    disabled: bool,
    onchange: EventHandler<String>,
) -> Element {
    let label = (!label.is_empty()).then_some(label);
    match input.kind {
        InputKind::Choice => rsx! {
            select {
                class: "{FIELD_CLASSES} w-full min-w-28",
                id,
                aria_label: label,
                disabled,
                value: "{value}",
                onchange: move |e| onchange.call(e.value()),
                option { value: "", selected: value.is_empty(), "\u{2014}" }
                for choice in input.choices.iter() {
                    option {
                        key: "{choice}",
                        value: "{choice}",
                        selected: *choice == value,
                        "{choice}"
                    }
                }
            }
        },
        InputKind::Number => rsx! {
            input {
                class: "{FIELD_CLASSES} w-full min-w-24 font-mono",
                id,
                aria_label: label,
                disabled,
                r#type: "number",
                value: "{value}",
                oninput: move |e| onchange.call(e.value()),
            }
        },
        InputKind::Text => rsx! {
            input {
                class: "{FIELD_CLASSES} w-full min-w-32 font-mono",
                id,
                aria_label: label,
                disabled,
                r#type: "text",
                value: "{value}",
                oninput: move |e| onchange.call(e.value()),
            }
        },
        InputKind::Secret => rsx! {
            textarea {
                class: "{FIELD_CLASSES} w-full min-w-32 font-mono [-webkit-text-security:disc]",
                id,
                aria_label: label,
                disabled,
                rows: 2,
                autocomplete: "off",
                spellcheck: false,
                value: "{value}",
                oninput: move |e| onchange.call(e.value()),
            }
        },
    }
}
