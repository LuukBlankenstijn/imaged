use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use derive_more::{Display, FromStr, IsVariant};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

#[derive(
    Debug, Clone, Copy, Display, FromStr, IsVariant, PartialEq, Eq, Serialize, Deserialize,
)]
#[display(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    Text,
    Number,
    Choice,
}

impl InputKind {
    pub fn from_string(value: String) -> Result<Self> {
        InputKind::from_str(&value).map_err(|e| {
            tracing::info!(err=%e, "failed to convert string to InputKind");
            AppError::Internal("conversion error".to_string())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptInput {
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub kind: InputKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub per_machine: bool,
    #[serde(default)]
    pub choices: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Script {
    pub id: i64,
    pub name: String,
    pub body: String,
    pub updated_at: DateTime<Utc>,
    pub inputs: Vec<ScriptInput>,
}

#[derive(Debug, Clone)]
pub struct NewScript {
    pub name: String,
    pub body: String,
    pub inputs: Vec<ScriptInput>,
}

pub type InputValues = BTreeMap<String, String>;

pub const MAX_SCRIPT_NAME_LEN: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct DeployInputs {
    pub shared: InputValues,
    pub per_machine: BTreeMap<i64, InputValues>,
}

impl ScriptInput {
    pub fn check(&self, value: &str) -> std::result::Result<(), String> {
        if value.contains('\0') {
            return Err(format!("input `{}` cannot contain NUL bytes", self.name));
        }
        match self.kind {
            InputKind::Text => Ok(()),
            InputKind::Number => match value.trim().parse::<f64>() {
                Ok(number) if number.is_finite() => Ok(()),
                _ => Err(format!("input `{}` must be a number", self.name)),
            },
            InputKind::Choice if self.choices.iter().any(|choice| choice == value) => Ok(()),
            InputKind::Choice => Err(format!(
                "input `{}` must be one of {}",
                self.name,
                self.choices.join(", ")
            )),
        }
    }

    pub fn value_from<'a>(&'a self, values: &'a InputValues) -> Option<&'a str> {
        values
            .get(&self.name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .or(self.default_value.as_deref())
    }

    pub fn validate(&self) -> Result {
        if !is_input_name(&self.name) {
            return Err(AppError::InvalidArgument(format!(
                "input name `{}` may only contain a-z, 0-9 and _",
                self.name
            )));
        }
        match self.kind {
            InputKind::Choice if self.choices.is_empty() => {
                return Err(AppError::InvalidArgument(format!(
                    "choice input `{}` needs at least one choice",
                    self.name
                )));
            }
            InputKind::Text | InputKind::Number if !self.choices.is_empty() => {
                return Err(AppError::InvalidArgument(format!(
                    "{} input `{}` cannot have choices",
                    self.kind, self.name
                )));
            }
            _ => {}
        }
        if let Some(duplicate) = first_duplicate(self.choices.iter().map(String::as_str)) {
            return Err(AppError::InvalidArgument(format!(
                "input `{}` lists choice `{duplicate}` twice",
                self.name
            )));
        }
        if let Some(default) = &self.default_value {
            self.check(default)
                .map_err(|e| AppError::InvalidArgument(format!("default value is invalid: {e}")))?;
        }
        Ok(())
    }
}

pub fn validate_script_name(name: &str) -> Result {
    if !is_script_name(name) {
        return Err(AppError::InvalidArgument(format!(
            "script name `{name}` must be 1 to {MAX_SCRIPT_NAME_LEN} letters, digits, -, _ or . and cannot start with ."
        )));
    }
    Ok(())
}

impl NewScript {
    pub fn validate(&self) -> Result {
        validate_script_name(&self.name)?;
        if self.body.trim().is_empty() {
            return Err(AppError::InvalidArgument(format!(
                "script `{}` has an empty body",
                self.name
            )));
        }
        for input in &self.inputs {
            input.validate()?;
        }
        if let Some(duplicate) = first_duplicate(self.inputs.iter().map(|i| i.name.as_str())) {
            return Err(AppError::InvalidArgument(format!(
                "script `{}` declares input `{duplicate}` twice",
                self.name
            )));
        }
        Ok(())
    }
}

pub fn resolve_inputs(
    scripts: &[Script],
    host_ids: &[i64],
    inputs: &DeployInputs,
) -> Result<BTreeMap<i64, InputValues>> {
    let declared: Vec<&ScriptInput> = scripts.iter().flat_map(|s| &s.inputs).collect();
    let declaration = |name: &str| declared.iter().find(|input| input.name == name);

    for name in inputs.shared.keys() {
        match declaration(name) {
            None => return Err(unknown_input(name)),
            Some(input) if input.per_machine => {
                return Err(AppError::InvalidArgument(format!(
                    "input `{name}` is set per machine"
                )));
            }
            Some(_) => {}
        }
    }
    for (host_id, values) in &inputs.per_machine {
        if !host_ids.contains(host_id) {
            return Err(AppError::InvalidArgument(format!(
                "host {host_id} is not part of this deployment"
            )));
        }
        for name in values.keys() {
            match declaration(name) {
                None => return Err(unknown_input(name)),
                Some(input) if !input.per_machine => {
                    return Err(AppError::InvalidArgument(format!(
                        "input `{name}` is shared by all machines"
                    )));
                }
                Some(_) => {}
            }
        }
    }
    for input in &declared {
        if declaration(&input.name).is_some_and(|first| first.per_machine != input.per_machine) {
            return Err(AppError::InvalidArgument(format!(
                "input `{}` is declared both shared and per machine",
                input.name
            )));
        }
    }

    let empty = InputValues::new();
    let mut resolved = BTreeMap::new();
    for &host_id in host_ids {
        let mut values = InputValues::new();
        for input in &declared {
            let source = if input.per_machine {
                inputs.per_machine.get(&host_id).unwrap_or(&empty)
            } else {
                &inputs.shared
            };
            match input.value_from(source) {
                Some(value) => {
                    input
                        .check(value)
                        .map_err(|e| AppError::InvalidArgument(format!("host {host_id}: {e}")))?;
                    values.insert(input.name.clone(), value.to_string());
                }
                None if input.required => {
                    return Err(AppError::InvalidArgument(format!(
                        "host {host_id}: input `{}` is required",
                        input.name
                    )));
                }
                None => {}
            }
        }
        resolved.insert(host_id, values);
    }
    Ok(resolved)
}

fn unknown_input(name: &str) -> AppError {
    AppError::InvalidArgument(format!("no script declares input `{name}`"))
}

fn is_input_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn is_script_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SCRIPT_NAME_LEN
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn first_duplicate<'a>(items: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = std::collections::HashSet::new();
    items.into_iter().find(|item| !seen.insert(*item))
}

#[async_trait::async_trait]
pub trait ScriptRepository: Send + Sync {
    async fn get_all(&self) -> Result<Vec<Script>>;

    async fn get(&self, id: i64) -> Result<Script>;

    async fn create(&self, script: &NewScript) -> Result<Script>;

    async fn update(&self, id: i64, script: &NewScript) -> Result<Script>;

    async fn delete(&self, id: i64) -> Result;

    async fn import_for_image(&self, image_id: i64, scripts: &[NewScript]) -> Result;

    async fn get_for_image(&self, image_id: i64) -> Result<Vec<Script>>;

    async fn set_for_image(&self, image_id: i64, script_ids: &[i64]) -> Result;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, kind: InputKind) -> ScriptInput {
        ScriptInput {
            name: name.to_string(),
            label: name.to_string(),
            kind,
            required: false,
            default_value: None,
            per_machine: false,
            choices: Vec::new(),
        }
    }

    fn script(inputs: Vec<ScriptInput>) -> Script {
        Script {
            id: 1,
            name: "setup".to_string(),
            body: "#!/bin/sh\n".to_string(),
            updated_at: Utc::now(),
            inputs,
        }
    }

    fn new_script(name: &str, inputs: Vec<ScriptInput>) -> NewScript {
        NewScript {
            name: name.to_string(),
            body: "echo hi".to_string(),
            inputs,
        }
    }

    fn values(pairs: &[(&str, &str)]) -> InputValues {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn rejected(result: Result<impl std::fmt::Debug>) -> String {
        match result {
            Err(AppError::InvalidArgument(message)) => message,
            other => panic!("expected an invalid argument, got {other:?}"),
        }
    }

    #[test]
    fn script_names_must_be_usable_as_archive_file_names() {
        for name in ["setup", "set-hostname_2.v1", "A1"] {
            new_script(name, vec![]).validate().unwrap();
        }
        new_script(&"a".repeat(MAX_SCRIPT_NAME_LEN), vec![])
            .validate()
            .unwrap();
        let too_long = "a".repeat(MAX_SCRIPT_NAME_LEN + 1);
        for name in ["", ".hidden", "with space", "a/b", "..", "naïve", &too_long] {
            rejected(new_script(name, vec![]).validate());
        }
    }

    #[test]
    fn a_script_needs_a_body() {
        let mut script = new_script("setup", vec![]);
        script.body = " \n".to_string();
        assert!(rejected(script.validate()).contains("empty body"));
    }

    #[test]
    fn input_names_are_lowercase_identifiers_declared_once() {
        rejected(new_script("s", vec![input("Hostname", InputKind::Text)]).validate());
        rejected(new_script("s", vec![input("host-name", InputKind::Text)]).validate());
        let twice = vec![
            input("site", InputKind::Text),
            input("site", InputKind::Number),
        ];
        assert!(rejected(new_script("s", twice).validate()).contains("twice"));
        new_script("s", vec![input("site_2", InputKind::Text)])
            .validate()
            .unwrap();
    }

    #[test]
    fn only_choice_inputs_have_choices_and_they_are_distinct() {
        rejected(new_script("s", vec![input("mode", InputKind::Choice)]).validate());

        let mut text = input("mode", InputKind::Text);
        text.choices = vec!["a".into()];
        rejected(new_script("s", vec![text]).validate());

        let mut repeated = input("mode", InputKind::Choice);
        repeated.choices = vec!["a".into(), "a".into()];
        rejected(new_script("s", vec![repeated]).validate());
    }

    #[test]
    fn a_default_value_must_be_valid_for_its_kind() {
        let mut number = input("count", InputKind::Number);
        number.default_value = Some("many".into());
        rejected(new_script("s", vec![number.clone()]).validate());
        number.default_value = Some("2.5".into());
        new_script("s", vec![number]).validate().unwrap();

        let mut choice = input("mode", InputKind::Choice);
        choice.choices = vec!["fast".into(), "safe".into()];
        choice.default_value = Some("slow".into());
        rejected(new_script("s", vec![choice]).validate());
    }

    #[test]
    fn shared_values_reach_every_host_and_per_machine_values_only_their_own() {
        let mut site = input("site", InputKind::Text);
        site.required = true;
        let mut hostname = input("hostname", InputKind::Text);
        hostname.per_machine = true;
        hostname.required = true;
        let scripts = [script(vec![site, hostname])];

        let resolved = resolve_inputs(
            &scripts,
            &[1, 2],
            &DeployInputs {
                shared: values(&[("site", "lab")]),
                per_machine: BTreeMap::from([
                    (1, values(&[("hostname", "pc-1")])),
                    (2, values(&[("hostname", "pc-2")])),
                ]),
            },
        )
        .unwrap();

        assert_eq!(
            resolved[&1],
            values(&[("site", "lab"), ("hostname", "pc-1")])
        );
        assert_eq!(
            resolved[&2],
            values(&[("site", "lab"), ("hostname", "pc-2")])
        );
    }

    #[test]
    fn a_missing_required_value_names_the_host_and_input() {
        let mut hostname = input("hostname", InputKind::Text);
        hostname.per_machine = true;
        hostname.required = true;
        let inputs = DeployInputs {
            shared: InputValues::new(),
            per_machine: BTreeMap::from([(1, values(&[("hostname", "pc-1")]))]),
        };

        let message = rejected(resolve_inputs(&[script(vec![hostname])], &[1, 2], &inputs));
        assert!(
            message.contains("host 2") && message.contains("hostname"),
            "{message}"
        );
    }

    #[test]
    fn empty_and_missing_values_fall_back_to_the_default_or_are_left_out() {
        let mut timezone = input("timezone", InputKind::Text);
        timezone.default_value = Some("UTC".into());
        timezone.required = true;
        let optional = input("note", InputKind::Text);

        let resolved = resolve_inputs(
            &[script(vec![timezone, optional])],
            &[1],
            &DeployInputs {
                shared: values(&[("timezone", ""), ("note", "")]),
                per_machine: BTreeMap::new(),
            },
        )
        .unwrap();

        assert_eq!(resolved[&1], values(&[("timezone", "UTC")]));
    }

    #[test]
    fn values_are_checked_against_their_kind() {
        let number = input("count", InputKind::Number);
        let mut choice = input("mode", InputKind::Choice);
        choice.choices = vec!["fast".into(), "safe".into()];
        let scripts = [script(vec![number, choice, input("note", InputKind::Text)])];
        let deploy = |pairs: &[(&str, &str)]| {
            resolve_inputs(
                &scripts,
                &[1],
                &DeployInputs {
                    shared: values(pairs),
                    per_machine: BTreeMap::new(),
                },
            )
        };

        assert!(rejected(deploy(&[("count", "ten")])).contains("number"));
        assert!(rejected(deploy(&[("count", "inf")])).contains("number"));
        assert!(rejected(deploy(&[("mode", "slow")])).contains("fast, safe"));
        assert!(rejected(deploy(&[("count", "1"), ("note", "a\0b")])).contains("NUL"));
        deploy(&[("count", "-3.5"), ("mode", "safe")]).unwrap();
    }

    #[test]
    fn values_must_match_a_declared_input_and_its_sharing() {
        let shared = input("site", InputKind::Text);
        let mut per_machine = input("hostname", InputKind::Text);
        per_machine.per_machine = true;
        let scripts = [script(vec![shared, per_machine])];
        let deploy = |shared: InputValues, per_machine: BTreeMap<i64, InputValues>| {
            resolve_inputs(
                &scripts,
                &[1],
                &DeployInputs {
                    shared,
                    per_machine,
                },
            )
        };

        assert!(rejected(deploy(values(&[("color", "red")]), BTreeMap::new())).contains("color"));
        rejected(deploy(values(&[("hostname", "pc")]), BTreeMap::new()));
        rejected(deploy(
            InputValues::new(),
            BTreeMap::from([(1, values(&[("site", "lab")]))]),
        ));
        assert!(
            rejected(deploy(
                InputValues::new(),
                BTreeMap::from([(9, values(&[("hostname", "pc")]))]),
            ))
            .contains("host 9")
        );
    }

    #[test]
    fn scripts_sharing_an_input_name_share_its_value_and_both_checks_apply() {
        let text = input("level", InputKind::Text);
        let number = input("level", InputKind::Number);
        let scripts = [script(vec![text]), script(vec![number])];
        let deploy = |level: &str| {
            resolve_inputs(
                &scripts,
                &[1],
                &DeployInputs {
                    shared: values(&[("level", level)]),
                    per_machine: BTreeMap::new(),
                },
            )
        };

        rejected(deploy("high"));
        assert_eq!(deploy("3").unwrap()[&1], values(&[("level", "3")]));

        let mut per_machine = input("level", InputKind::Text);
        per_machine.per_machine = true;
        let conflicting = [
            script(vec![input("level", InputKind::Text)]),
            script(vec![per_machine]),
        ];
        assert!(
            rejected(resolve_inputs(&conflicting, &[1], &DeployInputs::default()))
                .contains("both shared and per machine")
        );
    }
}
