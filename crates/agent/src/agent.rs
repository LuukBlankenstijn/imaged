use std::sync::Arc;

use crate::server::Server;
use crate::status::Status;

#[derive(Clone)]
pub struct Agent {
    pub server: Arc<Server>,
    pub status: Status,
}
