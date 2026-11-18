use std::collections::BTreeMap;

use imaged_model::script_env::{AGENT_VARIABLES, INPUT_PREFIX, PROXY_VARIABLES};

use crate::status::Identity;

pub const SYSTEM_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub fn script_env(
    path: &str,
    inputs: &BTreeMap<String, String>,
    identity: &Identity,
    proxy: Option<&str>,
    variables: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let ip = identity.ip.as_deref().unwrap_or_default();
    let agent_values = [path, "/root", &identity.mac, ip, &identity.server];
    let mut env: Vec<(String, String)> = AGENT_VARIABLES
        .iter()
        .zip(agent_values)
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    env.extend(
        variables
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    env.extend(inputs.iter().map(|(name, value)| {
        (
            format!("{INPUT_PREFIX}{}", name.to_ascii_uppercase()),
            value.clone(),
        )
    }));
    if let Some(proxy) = proxy {
        env.extend(
            PROXY_VARIABLES
                .iter()
                .map(|name| (name.to_string(), proxy.to_string())),
        );
    }
    env
}

pub fn script_command<'a>(body: &str, path: &'a str) -> Vec<&'a str> {
    if body.starts_with("#!") {
        vec![path]
    } else {
        vec!["/bin/sh", path]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT_PATH: &str = "/tmp/imaged-script";

    fn agent() -> Identity {
        Identity {
            mac: "52:54:00:12:34:56".to_string(),
            ip: Some("10.0.2.15".to_string()),
            server: "http://10.0.2.2:8080".to_string(),
        }
    }

    fn inputs(values: &[(&str, &str)]) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn lookup<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn inputs_and_the_agent_identity_reach_the_script_as_variables() {
        let env = script_env(
            SYSTEM_PATH,
            &inputs(&[("host_name", "pc-1"), ("site", "")]),
            &agent(),
            None,
            &BTreeMap::new(),
        );

        assert_eq!(lookup(&env, "IMAGED_INPUT_HOST_NAME"), Some("pc-1"));
        assert_eq!(lookup(&env, "IMAGED_INPUT_SITE"), Some(""));
        assert_eq!(lookup(&env, "IMAGED_MAC"), Some("52:54:00:12:34:56"));
        assert_eq!(lookup(&env, "IMAGED_IP"), Some("10.0.2.15"));
        assert_eq!(lookup(&env, "IMAGED_SERVER"), Some("http://10.0.2.2:8080"));
        assert_eq!(lookup(&env, "PATH"), Some(SYSTEM_PATH));
        assert_eq!(lookup(&env, "http_proxy"), None);
    }

    #[test]
    fn a_configured_proxy_is_exported_in_both_spellings() {
        let env = script_env(
            SYSTEM_PATH,
            &BTreeMap::new(),
            &agent(),
            Some("http://proxy:3128"),
            &BTreeMap::new(),
        );

        for name in PROXY_VARIABLES {
            assert_eq!(lookup(&env, name), Some("http://proxy:3128"), "{name}");
        }
    }

    #[test]
    fn variables_from_the_server_reach_every_script() {
        let variables = BTreeMap::from([("IMAGED_IMAGE_TOKEN".to_string(), "secret".to_string())]);
        let env = script_env(SYSTEM_PATH, &BTreeMap::new(), &agent(), None, &variables);

        assert_eq!(lookup(&env, "IMAGED_IMAGE_TOKEN"), Some("secret"));
    }

    #[test]
    fn a_shebang_picks_the_interpreter_and_anything_else_runs_in_sh() {
        assert_eq!(
            script_command("#!/usr/bin/env python3\nprint()", SCRIPT_PATH),
            [SCRIPT_PATH]
        );
        assert_eq!(
            script_command("echo hi", SCRIPT_PATH),
            ["/bin/sh", SCRIPT_PATH]
        );
        assert_eq!(
            script_command(" #!/bin/bash", SCRIPT_PATH),
            ["/bin/sh", SCRIPT_PATH]
        );
    }
}
