use std::collections::BTreeMap;

use dioxus::prelude::*;

use crate::api::scripts::get_image_scripts;
use crate::components::icons::Icon;
use crate::components::ui::{FIELD_CLASSES, Spinner};
use crate::model::{DeployInputs, Host, Image, InputKind, Script, ScriptInput};

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
    let defaults = |per_machine: bool| -> BTreeMap<String, String> {
        fields
            .iter()
            .filter(|f| f.per_machine == per_machine)
            .filter_map(|f| Some((f.name.clone(), f.default_value.clone()?)))
            .collect()
    };
    let per_host = defaults(true);
    DeployInputs {
        shared: defaults(false),
        per_machine: host_ids.iter().map(|&id| (id, per_host.clone())).collect(),
    }
}

pub(crate) fn submitted_inputs(form: &DeployInputs) -> DeployInputs {
    let filled = |values: &BTreeMap<String, String>| -> BTreeMap<String, String> {
        values
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    };
    DeployInputs {
        shared: filled(&form.shared),
        per_machine: form
            .per_machine
            .iter()
            .map(|(&host_id, values)| (host_id, filled(values)))
            .filter(|(_, values)| !values.is_empty())
            .collect(),
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
                                    FieldLabel { input: field.clone() }
                                    FieldControl {
                                        input: field.clone(),
                                        value: values.shared.get(&field.name).cloned().unwrap_or_default(),
                                        onchange: {
                                            let name = field.name.clone();
                                            move |value: String| {
                                                let mut inputs = inputs;
                                                if let Some(form) = inputs.write().as_mut() {
                                                    form.shared.insert(name.clone(), value);
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
                                                            input: field.clone(),
                                                            value: values
                                                                .per_machine
                                                                .get(&host.id)
                                                                .and_then(|v| v.get(&field.name))
                                                                .cloned()
                                                                .unwrap_or_default(),
                                                            onchange: {
                                                                let name = field.name.clone();
                                                                let host_id = host.id;
                                                                move |value: String| {
                                                                    let mut inputs = inputs;
                                                                    if let Some(form) = inputs.write().as_mut() {
                                                                        form.per_machine
                                                                            .entry(host_id)
                                                                            .or_default()
                                                                            .insert(name.clone(), value);
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
fn FieldLabel(input: ScriptInput) -> Element {
    let label = field_label(&input).to_string();
    rsx! {
        label { class: "mb-1 block font-mono text-[11px] uppercase tracking-wider text-fog-500",
            "{label}"
            if input.required {
                span { class: "ml-0.5 text-amber-500", "*" }
            }
        }
    }
}

#[component]
fn FieldControl(input: ScriptInput, value: String, onchange: EventHandler<String>) -> Element {
    match input.kind {
        InputKind::Choice => rsx! {
            select {
                class: "{FIELD_CLASSES} w-full min-w-28",
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
                r#type: "number",
                value: "{value}",
                oninput: move |e| onchange.call(e.value()),
            }
        },
        InputKind::Text => rsx! {
            input {
                class: "{FIELD_CLASSES} w-full min-w-32 font-mono",
                r#type: "text",
                value: "{value}",
                oninput: move |e| onchange.call(e.value()),
            }
        },
    }
}
