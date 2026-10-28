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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPartition {
    pub number: i64,
    pub partuuid: String,
    pub partlabel: Option<String>,
    pub label: Option<String>,
    pub uuid: Option<String>,
    pub fstype: Option<String>,
    pub size_bytes: u64,
}
