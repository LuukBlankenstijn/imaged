use std::collections::BTreeMap;

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

pub type InputValues = BTreeMap<String, String>;

pub const MAX_SCRIPT_NAME_LEN: usize = 64;

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

pub fn validate_script(name: &str, body: &str, inputs: &[ScriptInput]) -> Result {
    validate_script_name(name)?;
    if body.trim().is_empty() {
        return Err(AppError::InvalidArgument(format!(
            "script `{name}` has an empty body"
        )));
    }
    for input in inputs {
        input.validate()?;
    }
    if let Some(duplicate) = first_duplicate(inputs.iter().map(|i| i.name.as_str())) {
        return Err(AppError::InvalidArgument(format!(
            "script `{name}` declares input `{duplicate}` twice"
        )));
    }
    Ok(())
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

    fn validate(name: &str, inputs: &[ScriptInput]) -> Result {
        validate_script(name, "echo hi", inputs)
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
            validate(name, &[]).unwrap();
        }
        validate(&"a".repeat(MAX_SCRIPT_NAME_LEN), &[]).unwrap();
        let too_long = "a".repeat(MAX_SCRIPT_NAME_LEN + 1);
        for name in ["", ".hidden", "with space", "a/b", "..", "naïve", &too_long] {
            rejected(validate(name, &[]));
        }
    }

    #[test]
    fn a_script_needs_a_body() {
        assert!(rejected(validate_script("setup", " \n", &[])).contains("empty body"));
    }

    #[test]
    fn input_names_are_lowercase_identifiers_declared_once() {
        rejected(validate("s", &[input("Hostname", InputKind::Text)]));
        rejected(validate("s", &[input("host-name", InputKind::Text)]));
        let twice = [
            input("site", InputKind::Text),
            input("site", InputKind::Number),
        ];
        assert!(rejected(validate("s", &twice)).contains("twice"));
        validate("s", &[input("site_2", InputKind::Text)]).unwrap();
    }

    #[test]
    fn only_choice_inputs_have_choices_and_they_are_distinct() {
        rejected(validate("s", &[input("mode", InputKind::Choice)]));

        let mut text = input("mode", InputKind::Text);
        text.choices = vec!["a".into()];
        rejected(validate("s", &[text]));

        let mut repeated = input("mode", InputKind::Choice);
        repeated.choices = vec!["a".into(), "a".into()];
        rejected(validate("s", &[repeated]));
    }

    #[test]
    fn a_default_value_must_be_valid_for_its_kind() {
        let mut number = input("count", InputKind::Number);
        number.default_value = Some("many".into());
        rejected(validate("s", &[number.clone()]));
        number.default_value = Some("2.5".into());
        validate("s", &[number]).unwrap();

        let mut choice = input("mode", InputKind::Choice);
        choice.choices = vec!["fast".into(), "safe".into()];
        choice.default_value = Some("slow".into());
        rejected(validate("s", &[choice]));
    }
}
