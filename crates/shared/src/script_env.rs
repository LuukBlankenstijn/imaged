pub const INPUT_PREFIX: &str = "IMAGED_INPUT_";
pub const AGENT_VARIABLES: [&str; 5] = ["PATH", "HOME", "IMAGED_MAC", "IMAGED_IP", "IMAGED_SERVER"];
pub const PROXY_VARIABLES: [&str; 4] = ["http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY"];

pub fn check_variable(name: &str, value: &str) -> Result<(), String> {
    let valid_name = name
        .bytes()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if !valid_name {
        return Err(format!(
            "script variable `{name}` may only contain letters, digits and _ and cannot start with a digit"
        ));
    }
    if name.starts_with(INPUT_PREFIX)
        || AGENT_VARIABLES.contains(&name)
        || PROXY_VARIABLES.contains(&name)
    {
        return Err(format!("script variable `{name}` is set by imaged itself"));
    }
    if value.contains('\0') {
        return Err(format!("script variable `{name}` cannot contain NUL bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_variable;

    #[test]
    fn names_imaged_sets_itself_are_refused() {
        for name in ["PATH", "IMAGED_MAC", "IMAGED_INPUT_HOSTNAME", "https_proxy"] {
            assert!(check_variable(name, "x").is_err(), "{name}");
        }
    }

    #[test]
    fn names_must_be_shell_identifiers_and_values_free_of_nul() {
        assert!(check_variable("IMAGED_IMAGE_TOKEN", "secret").is_ok());
        assert!(check_variable("_private", "").is_ok());
        assert!(check_variable("", "x").is_err());
        assert!(check_variable("1ST", "x").is_err());
        assert!(check_variable("WITH-DASH", "x").is_err());
        assert!(check_variable("TOKEN", "a\0b").is_err());
    }
}
