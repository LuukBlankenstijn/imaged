use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crate::image::ImagePartition;

pub const MAC_HEADER: &str = "X-Agent-Mac";
pub const IP_HEADER: &str = "X-Agent-Ip";

pub mod route {
    pub const STREAM: &str = "/api/client/stream";
    pub const DISCONNECT: &str = "/api/client/stream/disconnect";
    pub const FINISHED: &str = "/api/client/tasks/{task_id}/finished";
    pub const FAILED: &str = "/api/client/tasks/{task_id}/failed";
    pub const PARTITION_TABLE: &str = "/api/client/tasks/{task_id}/parttable";
    pub const PARTITIONS: &str = "/api/client/tasks/{task_id}/partitions";
    pub const PARTITION_DATA: &str =
        "/api/client/tasks/{task_id}/partitions/{partition_number}/data";
    pub const ROOT_PARTITION: &str =
        "/api/client/tasks/{task_id}/root-partition/{partition_number}";
    pub const SCRIPTS: &str = "/api/client/tasks/{task_id}/scripts";
    pub const SCRIPT_OUTPUT: &str = "/api/client/tasks/{task_id}/scripts/{position}/output";
    pub const SCRIPT_FINISHED: &str = "/api/client/tasks/{task_id}/scripts/{position}/finished";
}

pub mod path {
    use super::route;

    fn task(route: &str, task_id: i64) -> String {
        route.replace("{task_id}", &task_id.to_string())
    }

    fn partition(route: &str, task_id: i64, partition_number: i64) -> String {
        task(route, task_id).replace("{partition_number}", &partition_number.to_string())
    }

    fn script(route: &str, task_id: i64, position: i64) -> String {
        task(route, task_id).replace("{position}", &position.to_string())
    }

    pub fn finished(task_id: i64) -> String {
        task(route::FINISHED, task_id)
    }

    pub fn failed(task_id: i64) -> String {
        task(route::FAILED, task_id)
    }

    pub fn partition_table(task_id: i64) -> String {
        task(route::PARTITION_TABLE, task_id)
    }

    pub fn partitions(task_id: i64) -> String {
        task(route::PARTITIONS, task_id)
    }

    pub fn partition_data(task_id: i64, partition_number: i64) -> String {
        partition(route::PARTITION_DATA, task_id, partition_number)
    }

    pub fn root_partition(task_id: i64, partition_number: i64) -> String {
        partition(route::ROOT_PARTITION, task_id, partition_number)
    }

    pub fn scripts(task_id: i64) -> String {
        task(route::SCRIPTS, task_id)
    }

    pub fn script_output(task_id: i64, position: i64) -> String {
        script(route::SCRIPT_OUTPUT, task_id, position)
    }

    pub fn script_finished(task_id: i64, position: i64) -> String {
        script(route::SCRIPT_FINISHED, task_id, position)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostInstall {
    pub root_partition: Option<i64>,
    pub proxy: Option<String>,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    pub scripts: Vec<PostInstallScript>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostInstallScript {
    pub position: i64,
    pub name: String,
    pub body: String,
    pub inputs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamQuery {
    pub disk_size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionUpload {
    pub filesystem: String,
    pub size: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFailure {
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptFinished {
    pub exit_code: Option<i64>,
}
