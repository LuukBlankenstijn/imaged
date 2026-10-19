use dioxus::prelude::*;

use crate::api::scripts::{create_script, delete_script, get_all_scripts, update_script};
use crate::components::icons::Icon;
use crate::components::modal::{ConfirmDialog, Modal};
use crate::components::toast::{toast_error, toast_success};
use crate::components::ui::{
    Button, ButtonVariant, Card, EmptyState, FIELD_CLASSES, PageHeader, Spinner,
};
use crate::format::format_relative;
use crate::model::{InputKind, Script, ScriptInput, ScriptRequest, UpdateScriptRequest};

#[derive(Clone, PartialEq)]
enum Editing {
    New,
    Existing(Script),
}

#[component]
pub fn Scripts() -> Element {
    let mut scripts = use_resource(|| async move { get_all_scripts().await });
    let mut editing = use_signal(|| None::<Editing>);

    rsx! {
        PageHeader {
            title: "Scripts",
            subtitle: "Post-install scripts run inside the restored system after a deploy",
            Button { onclick: move |_| editing.set(Some(Editing::New)),
                Icon { name: "plus", class: "w-3.5 h-3.5" }
                "New script"
            }
        }
        Card { class: "overflow-hidden",
            match &*scripts.read() {
                Some(Ok(list)) if list.is_empty() => rsx! {
                    EmptyState {
                        title: "No scripts yet",
                        hint: "Create a script, then attach it to an image to run it after every deploy.",
                    }
                },
                Some(Ok(list)) => rsx! {
                    table { class: "w-full text-sm",
                        thead {
                            tr { class: "text-fog-500",
                                th { class: "px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider", "Script" }
                                th { class: "px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider", "Inputs" }
                                th { class: "px-4 py-2.5 text-left text-xs font-medium uppercase tracking-wider", "Updated" }
                                th { class: "px-4 py-2.5 text-right text-xs font-medium uppercase tracking-wider" }
                            }
                        }
                        tbody {
                            for (i , s) in list.iter().enumerate() {
                                ScriptRow {
                                    key: "{s.id}",
                                    script: s.clone(),
                                    index: i,
                                    onedit: move |script| editing.set(Some(Editing::Existing(script))),
                                    on_changed: move |_| scripts.restart(),
                                }
                            }
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    EmptyState { title: "Failed to load scripts", hint: e.to_string() }
                },
                None => rsx! {
                    div { class: "flex justify-center py-16", Spinner {} }
                },
            }
        }
        if let Some(target) = editing() {
            ScriptEditor {
                script: match target {
                    Editing::New => None,
                    Editing::Existing(script) => Some(script),
                },
                onclose: move |_| editing.set(None),
                on_saved: move |_| {
                    editing.set(None);
                    scripts.restart();
                },
            }
        }
    }
}

#[component]
fn ScriptRow(
    script: Script,
    index: usize,
    onedit: EventHandler<Script>,
    on_changed: EventHandler<()>,
) -> Element {
    let mut delete_open = use_signal(|| false);
    let id = script.id;
    let name = script.name.clone();
    let input_count = script.inputs.len();
    let updated = format_relative(script.updated_at);
    let delay = index * 40;

    rsx! {
        tr {
            class: "border-t border-line/60 transition-colors hover:bg-ink-800/40 rise",
            style: "--d: {delay}ms",
            td { class: "px-4 py-3",
                div { class: "flex items-center gap-2.5",
                    Icon { name: "script", class: "w-4 h-4 text-amber-500" }
                    span { class: "font-mono text-fog-100", "{name}" }
                }
            }
            td { class: "px-4 py-3 font-mono text-xs text-fog-400", "{input_count}" }
            td { class: "px-4 py-3 font-mono text-xs text-fog-500", "{updated}" }
            td { class: "px-4 py-3",
                div { class: "flex justify-end gap-1.5",
                    Button {
                        variant: ButtonVariant::Ghost,
                        class: "px-2",
                        onclick: move |_| onedit.call(script.clone()),
                        Icon { name: "edit", class: "w-4 h-4" }
                    }
                    Button {
                        variant: ButtonVariant::Danger,
                        class: "px-2",
                        onclick: move |_| delete_open.set(true),
                        Icon { name: "trash", class: "w-4 h-4" }
                    }
                }
                ConfirmDialog {
                    open: delete_open(),
                    title: "Delete script",
                    message: format!("Delete {name}? Images that still run it must drop it first."),
                    confirm_label: "Delete",
                    danger: true,
                    onconfirm: move |_| {
                        delete_open.set(false);
                        spawn(async move {
                            match delete_script(id).await {
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InputDraft {
    pub name: String,
    pub label: String,
    pub description: String,
    pub kind: InputKind,
    pub required: bool,
    pub default_value: String,
    pub per_machine: bool,
    pub choices: String,
}

impl InputDraft {
    fn new() -> Self {
        Self {
            name: String::new(),
            label: String::new(),
            description: String::new(),
            kind: InputKind::Text,
            required: false,
            default_value: String::new(),
            per_machine: false,
            choices: String::new(),
        }
    }
}

impl From<&ScriptInput> for InputDraft {
    fn from(input: &ScriptInput) -> Self {
        Self {
            name: input.name.clone(),
            label: input.label.clone(),
            description: input.description.clone().unwrap_or_default(),
            kind: input.kind,
            required: input.required,
            default_value: input.default_value.clone().unwrap_or_default(),
            per_machine: input.per_machine,
            choices: input.choices.join(", "),
        }
    }
}

impl From<&InputDraft> for ScriptInput {
    fn from(draft: &InputDraft) -> Self {
        let default_value = match draft.kind {
            InputKind::Secret => "",
            InputKind::Text | InputKind::Number | InputKind::Choice => draft.default_value.trim(),
        };
        let choices = match draft.kind {
            InputKind::Choice => draft
                .choices
                .split(',')
                .map(str::trim)
                .filter(|choice| !choice.is_empty())
                .map(str::to_string)
                .collect(),
            InputKind::Text | InputKind::Number | InputKind::Secret => Vec::new(),
        };
        Self {
            name: draft.name.trim().to_string(),
            label: draft.label.trim().to_string(),
            description: Some(draft.description.clone()),
            kind: draft.kind,
            required: draft.required,
            default_value: (!default_value.is_empty()).then(|| default_value.to_string()),
            per_machine: draft.per_machine,
            choices,
        }
    }
}

fn kind_value(kind: InputKind) -> &'static str {
    match kind {
        InputKind::Text => "text",
        InputKind::Number => "number",
        InputKind::Choice => "choice",
        InputKind::Secret => "secret",
    }
}

fn parse_kind(value: &str) -> InputKind {
    match value {
        "number" => InputKind::Number,
        "choice" => InputKind::Choice,
        "secret" => InputKind::Secret,
        _ => InputKind::Text,
    }
}

#[component]
fn ScriptEditor(
    script: Option<Script>,
    onclose: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let id = script.as_ref().map(|s| s.id);
    let mut name = use_signal(|| script.as_ref().map(|s| s.name.clone()).unwrap_or_default());
    let mut body = use_signal(|| script.as_ref().map(|s| s.body.clone()).unwrap_or_default());
    let mut inputs = use_signal(|| {
        script
            .as_ref()
            .map(|s| s.inputs.iter().map(InputDraft::from).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let mut saving = use_signal(|| false);
    let title = if id.is_some() {
        "Edit script"
    } else {
        "New script"
    };
    let drafts = inputs();
    let count = drafts.len();

    rsx! {
        Modal { open: true, wide: true, dismissible: false, onclose: move |_| onclose.call(()), title,
            div { class: "flex flex-col gap-4",
                div {
                    label { class: "mb-1 block font-mono text-[11px] uppercase tracking-wider text-fog-500",
                        "Name"
                    }
                    input {
                        class: "{FIELD_CLASSES} w-full max-w-sm font-mono",
                        placeholder: "set-hostname",
                        maxlength: 64,
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label { class: "mb-1 block font-mono text-[11px] uppercase tracking-wider text-fog-500",
                        "Body"
                    }
                    textarea {
                        class: "{FIELD_CLASSES} block w-full font-mono text-xs leading-relaxed",
                        rows: 14,
                        spellcheck: false,
                        placeholder: "#!/bin/sh\nset -eu\necho \"$IMAGED_INPUT_HOSTNAME\" > /etc/hostname",
                        value: "{body}",
                        oninput: move |e| body.set(e.value()),
                    }
                    p { class: "mt-1 text-xs text-fog-600",
                        "Runs chrooted in the restored root partition, with /bin/sh unless it starts with #!. Inputs arrive as IMAGED_INPUT_<NAME>; IMAGED_MAC, IMAGED_IP and IMAGED_SERVER are always set."
                    }
                }
                div {
                    div { class: "mb-1 flex items-center justify-between",
                        span { class: "font-mono text-[11px] uppercase tracking-wider text-fog-500",
                            "Inputs"
                        }
                        Button {
                            variant: ButtonVariant::Subtle,
                            onclick: move |_| inputs.write().push(InputDraft::new()),
                            Icon { name: "plus", class: "w-3.5 h-3.5" }
                            "Add input"
                        }
                    }
                    if drafts.is_empty() {
                        p { class: "rounded-md border border-line bg-ink-900/60 px-3 py-3 text-xs text-fog-600",
                            "No inputs. The script gets the same environment on every machine."
                        }
                    } else {
                        div { class: "overflow-x-auto rounded-md border border-line",
                            table { class: "w-full text-sm",
                                thead {
                                    tr { class: "text-fog-500",
                                        th { class: "px-2 py-2 text-left text-[11px] font-medium uppercase tracking-wider", "Name" }
                                        th { class: "px-2 py-2 text-left text-[11px] font-medium uppercase tracking-wider", "Label & description" }
                                        th { class: "px-2 py-2 text-left text-[11px] font-medium uppercase tracking-wider", "Kind" }
                                        th { class: "px-2 py-2 text-left text-[11px] font-medium uppercase tracking-wider", "Default" }
                                        th { class: "px-2 py-2 text-left text-[11px] font-medium uppercase tracking-wider", "Choices" }
                                        th { class: "px-2 py-2 text-center text-[11px] font-medium uppercase tracking-wider", "Required" }
                                        th { class: "px-2 py-2 text-center text-[11px] font-medium uppercase tracking-wider", "Per host" }
                                        th { class: "px-2 py-2" }
                                    }
                                }
                                tbody {
                                    for (index , draft) in drafts.iter().enumerate() {
                                        InputRow {
                                            key: "{index}",
                                            index,
                                            count,
                                            draft: draft.clone(),
                                            inputs,
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "mt-6 flex justify-end gap-2",
                Button { variant: ButtonVariant::Ghost, onclick: move |_| onclose.call(()), "Cancel" }
                Button {
                    disabled: saving() || name().trim().is_empty(),
                    onclick: move |_| {
                        let request = ScriptRequest {
                            name: name().trim().to_string(),
                            body: body(),
                            inputs: inputs.read().iter().map(ScriptInput::from).collect(),
                        };
                        saving.set(true);
                        spawn(async move {
                            let result = match id {
                                Some(id) => update_script(UpdateScriptRequest { id, script: request }).await,
                                None => create_script(request).await,
                            };
                            saving.set(false);
                            match result {
                                Ok(script) => {
                                    toast_success("Script saved", script.name);
                                    on_saved.call(());
                                }
                                Err(err) => toast_error("Save failed", err.to_string()),
                            }
                        });
                    },
                    "Save"
                }
            }
        }
    }
}

#[component]
fn InputRow(
    index: usize,
    count: usize,
    draft: InputDraft,
    inputs: Signal<Vec<InputDraft>>,
) -> Element {
    let mut inputs = inputs;
    let is_choice = draft.kind == InputKind::Choice;

    rsx! {
        tr { class: "border-t border-line/60 align-top",
            td { class: "px-2 py-1.5",
                input {
                    class: "{FIELD_CLASSES} w-32 font-mono text-xs",
                    placeholder: "hostname",
                    value: "{draft.name}",
                    oninput: move |e| inputs.write()[index].name = e.value(),
                }
            }
            td { class: "px-2 py-1.5",
                div { class: "flex flex-col gap-1",
                    input {
                        class: "{FIELD_CLASSES} w-48 text-xs",
                        placeholder: "{draft.name}",
                        value: "{draft.label}",
                        oninput: move |e| inputs.write()[index].label = e.value(),
                    }
                    textarea {
                        class: "{FIELD_CLASSES} block w-48 text-xs leading-snug",
                        rows: 2,
                        maxlength: 1000,
                        placeholder: "Description (optional)",
                        value: "{draft.description}",
                        oninput: move |e| inputs.write()[index].description = e.value(),
                    }
                }
            }
            td { class: "px-2 py-1.5",
                select {
                    class: "{FIELD_CLASSES} text-xs",
                    value: kind_value(draft.kind),
                    onchange: move |e| inputs.write()[index].kind = parse_kind(&e.value()),
                    option { value: "text", selected: draft.kind == InputKind::Text, "text" }
                    option { value: "number", selected: draft.kind == InputKind::Number, "number" }
                    option { value: "choice", selected: draft.kind == InputKind::Choice, "choice" }
                    option { value: "secret", selected: draft.kind == InputKind::Secret, "secret" }
                }
            }
            td { class: "px-2 py-1.5",
                input {
                    class: "{FIELD_CLASSES} w-24 font-mono text-xs",
                    disabled: draft.kind == InputKind::Secret,
                    placeholder: if draft.kind == InputKind::Secret { "none" } else { "" },
                    value: if draft.kind == InputKind::Secret { "" } else { "{draft.default_value}" },
                    oninput: move |e| inputs.write()[index].default_value = e.value(),
                }
            }
            td { class: "px-2 py-1.5",
                if is_choice {
                    input {
                        class: "{FIELD_CLASSES} w-32 font-mono text-xs",
                        placeholder: "a, b, c",
                        value: "{draft.choices}",
                        oninput: move |e| inputs.write()[index].choices = e.value(),
                    }
                } else {
                    span { class: "px-1 text-xs text-fog-600", "\u{2014}" }
                }
            }
            td { class: "px-2 py-1.5 text-center",
                input {
                    r#type: "checkbox",
                    class: "accent-amber-500",
                    checked: draft.required,
                    onchange: move |e| inputs.write()[index].required = e.checked(),
                }
            }
            td { class: "px-2 py-1.5 text-center",
                input {
                    r#type: "checkbox",
                    class: "accent-amber-500",
                    checked: draft.per_machine,
                    onchange: move |e| inputs.write()[index].per_machine = e.checked(),
                }
            }
            td { class: "px-2 py-1.5",
                div { class: "flex justify-end",
                    Button {
                        variant: ButtonVariant::Subtle,
                        class: "px-1.5",
                        disabled: index == 0,
                        onclick: move |_| inputs.write().swap(index, index - 1),
                        Icon { name: "chevron", class: "w-3.5 h-3.5 rotate-180" }
                    }
                    Button {
                        variant: ButtonVariant::Subtle,
                        class: "px-1.5",
                        disabled: index + 1 == count,
                        onclick: move |_| inputs.write().swap(index, index + 1),
                        Icon { name: "chevron", class: "w-3.5 h-3.5" }
                    }
                    Button {
                        variant: ButtonVariant::Subtle,
                        class: "px-1.5 hover:text-bad",
                        onclick: move |_| {
                            inputs.write().remove(index);
                        },
                        Icon { name: "close", class: "w-3.5 h-3.5" }
                    }
                }
            }
        }
    }
}
