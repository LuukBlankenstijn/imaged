use derive_more::{Constructor, Display};

use crate::agent::Agent;
use crate::task::implementations::ClientTaskExt;

#[derive(Clone, Display, Constructor)]
#[display("reboot task")]
pub struct RebootTask {
    agent: Agent,
    task_id: i64,
}

impl ClientTaskExt for RebootTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    fn agent(&self) -> &Agent {
        &self.agent
    }

    async fn run(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        super::finish_and_reboot(self).await
    }
}
