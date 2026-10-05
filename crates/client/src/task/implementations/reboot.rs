use derive_more::{Constructor, Display};

use crate::task::implementations::ClientTaskExt;

#[derive(Clone, Display, Constructor)]
#[display("reboot task")]
pub struct RebootTask {
    task_id: i64,
}

impl ClientTaskExt for RebootTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        super::finish_and_reboot(self).await
    }
}
