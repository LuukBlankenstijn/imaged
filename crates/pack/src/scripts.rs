use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use imaged_archive::Script;
use imaged_model::script::{ScriptInput, validate_script};

pub async fn load(paths: &[PathBuf]) -> anyhow::Result<Vec<Script>> {
    let mut names = HashSet::with_capacity(paths.len());
    let mut scripts = Vec::with_capacity(paths.len());
    for path in paths {
        let script = load_one(path).await?;
        if !names.insert(script.name.clone()) {
            bail!("script `{}` is given more than once", script.name);
        }
        scripts.push(script);
    }
    Ok(scripts)
}

async fn load_one(path: &Path) -> anyhow::Result<Script> {
    if path.extension() != Some(OsStr::new("sh")) {
        bail!("script {} must be a <name>.sh file", path.display());
    }
    let name = path
        .file_stem()
        .and_then(OsStr::to_str)
        .with_context(|| format!("script {} has no UTF-8 name", path.display()))?
        .to_owned();
    let body = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("reading script {}", path.display()))?;
    let inputs = read_inputs(&path.with_extension("inputs.json")).await?;
    validate_script(&name, &body, &inputs)
        .with_context(|| format!("script {} is invalid", path.display()))?;
    Ok(Script { name, body, inputs })
}

async fn read_inputs(path: &Path) -> anyhow::Result<Vec<ScriptInput>> {
    match tokio::fs::read(path).await {
        Ok(json) => serde_json::from_slice(&json)
            .with_context(|| format!("parsing script inputs {}", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading script inputs {}", path.display())),
    }
}
