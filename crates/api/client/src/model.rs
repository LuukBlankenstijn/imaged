use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePartition {
    pub id: i64,
    pub partition_number: i64,
    pub fstype: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostInstall {
    pub root_partition: Option<i64>,
    pub proxy: Option<String>,
    pub scripts: Vec<PostInstallScript>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostInstallScript {
    pub position: i64,
    pub name: String,
    pub body: String,
    pub inputs: BTreeMap<String, String>,
}
