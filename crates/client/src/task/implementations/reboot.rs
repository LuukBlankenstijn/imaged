use derive_more::{Constructor, Display};

use crate::{sys, task::implementations::ClientTaskExt};

#[derive(Clone, Display, Constructor)]
#[display("reboot task")]
pub(crate) struct RebootTask {
    task_id: i64,
}

impl ClientTaskExt for RebootTask {
    async fn finalize(&self, api: &crate::transport::ApiClient) -> anyhow::Result<()> {
        api.mark_task_finished(self.task_id).await?;
        sys::reboot()
    }
}
