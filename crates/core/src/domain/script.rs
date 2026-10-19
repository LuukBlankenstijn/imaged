use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use imaged_model::script::{InputValues, ScriptInput, validate_script};

use imaged_model::error::{AppError, Result};
use imaged_model::script::ResolvedInputs;

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

impl NewScript {
    pub fn validate(&self) -> Result {
        validate_script(&self.name, &self.body, &self.inputs)
    }
}

pub fn resolve_inputs(
    scripts: &[Script],
    host_ids: &[i64],
    inputs: &ResolvedInputs,
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
    use imaged_model::script::InputKind;

    use super::*;

    fn input(name: &str, kind: InputKind) -> ScriptInput {
        ScriptInput {
            name: name.to_string(),
            label: name.to_string(),
            description: None,
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
            &ResolvedInputs {
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
        let inputs = ResolvedInputs {
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
            &ResolvedInputs {
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
                &ResolvedInputs {
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
                &ResolvedInputs {
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
                &ResolvedInputs {
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
            rejected(resolve_inputs(
                &conflicting,
                &[1],
                &ResolvedInputs::default()
            ))
            .contains("both shared and per machine")
        );
    }
}
