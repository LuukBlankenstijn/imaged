use derive_more::Constructor;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Constructor, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConnectionEvent {
    pub id: i64,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionChange {
    Connected(Vec<i64>),
    Changed(HostConnectionEvent),
}
