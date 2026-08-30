use derive_more::{Constructor, Display};

use crate::{sys, task::implementations::ClientTaskExt};

#[derive(Clone, Display, Constructor)]
#[display("reboot task")]
pub struct RebootTask {
    task_id: i64,
}

impl ClientTaskExt for RebootTask {
    async fn finalize(&self) -> anyhow::Result<()> {
        api::task::mark_finished(self.task_id).await?;
        // best effort disconnect
        let _ = api::event::disconnect().await;
        sys::reboot()
    }
}
