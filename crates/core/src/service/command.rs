use std::collections::BTreeMap;

use imaged_model::command::{CommandRun, CommandScript, HostCommandResult};
use imaged_model::error::{AppError, Result};

use crate::domain::host::{Host, HostRepository};
use crate::domain::script::{ScriptRepository, resolve_inputs};
use crate::registry::HostRegistry;

pub struct CommandService<'a> {
    pub registry: &'a HostRegistry,
    pub hosts: &'a dyn HostRepository,
    pub scripts: &'a dyn ScriptRepository,
    pub variables: &'a BTreeMap<String, String>,
    pub proxy: Option<&'a str>,
}

impl CommandService<'_> {
    pub async fn run(&self, host_ids: &[i64], run: CommandRun) -> Result<Vec<HostCommandResult>> {
        let hosts = self.targets(host_ids).await?;
        let script = |body: String, inputs| CommandScript {
            body,
            inputs,
            variables: self.variables.clone(),
            proxy: self.proxy.map(str::to_owned),
        };
        let scripts = match run {
            CommandRun::Command(command) => hosts
                .iter()
                .map(|host| (host.id, script(command.clone(), BTreeMap::new())))
                .collect(),
            CommandRun::Script { id, inputs } => {
                let library = self.scripts.get(id).await?;
                if !library.kind.is_client() {
                    return Err(AppError::InvalidArgument(format!(
                        "script `{}` is a post-install script, only client scripts run on connected hosts",
                        library.name
                    )));
                }
                let resolved = inputs.resolve(self.variables)?;
                let mut values = resolve_inputs(std::slice::from_ref(&library), &hosts, &resolved)?;
                hosts
                    .iter()
                    .map(|host| {
                        let inputs = values.remove(&host.id).unwrap_or_default();
                        (host.id, script(library.body.clone(), inputs))
                    })
                    .collect()
            }
        };
        Ok(self.registry.run(scripts).await)
    }

    async fn targets(&self, host_ids: &[i64]) -> Result<Vec<Host>> {
        let mut known: BTreeMap<i64, Host> = self
            .hosts
            .get_all(None)
            .await?
            .into_iter()
            .map(|host| (host.id, host))
            .collect();
        let mut targets: Vec<Host> = Vec::with_capacity(host_ids.len());
        for id in host_ids {
            if targets.iter().any(|host| host.id == *id) {
                continue;
            }
            let host = known
                .remove(id)
                .ok_or_else(|| AppError::NotFound(format!("host {id}")))?;
            targets.push(host);
        }
        Ok(targets)
    }
}
