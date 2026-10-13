use std::sync::Arc;

use derive_more::{Constructor, Display};

use crate::server::Server;
use crate::task::implementations::ClientTaskExt;

#[derive(Clone, Display, Constructor)]
#[display("reboot task")]
pub struct RebootTask {
    server: Arc<Server>,
    task_id: i64,
}

impl ClientTaskExt for RebootTask {
    fn task_id(&self) -> i64 {
        self.task_id
    }

    fn server(&self) -> &Server {
        &self.server
    }

    async fn finalize(&self) -> anyhow::Result<()> {
        super::finish_and_reboot(self).await
    }
}
