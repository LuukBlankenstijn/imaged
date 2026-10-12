use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: i64,
    pub mac_address: String,
    pub name: String,
    pub disk_size_bytes: u64,
    pub ip: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConnectionEvent {
    pub id: i64,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionUpdate {
    Connected(Vec<i64>),
    Changed(HostConnectionEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticastProgress {
    pub task_id: i64,
    pub fraction: f64,
    pub bytes_per_second: f64,
    pub receivers: usize,
    pub step: usize,
    pub steps: usize,
    pub eta_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageStatus {
    Empty,
    Capturing,
    Importing,
    Ready,
    Faulted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePartition {
    pub id: i64,
    pub partition_number: i64,
    pub fstype: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub id: i64,
    pub name: String,
    pub captured_at: Option<i64>,
    pub status: ImageStatus,
    pub error_message: Option<String>,
    pub partitions: Vec<ImagePartition>,
    pub root_partition: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    Capture,
    Deploy,
    Multicast,
    Reboot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Pending,
    Running,
    Done,
    Cancelled,
    Failed,
    Partial,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskHost {
    pub host_id: i64,
    pub state: TaskState,
    pub error: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub r#type: TaskType,
    pub hosts: Vec<TaskHost>,
    pub image_id: Option<i64>,
    pub state: TaskState,
    pub created_at: i64,
    pub image_name: Option<String>,
    pub image_deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateName {
    pub id: i64,
    pub new_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployRequest {
    pub id: i64,
    pub image_id: i64,
    #[serde(default)]
    pub inputs: DeployInputs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MulticastRequest {
    pub host_ids: Vec<i64>,
    pub image_id: i64,
    #[serde(default)]
    pub inputs: DeployInputs,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployInputs {
    #[serde(default)]
    pub shared: BTreeMap<String, String>,
    #[serde(default)]
    pub per_machine: BTreeMap<i64, BTreeMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateImageRequest {
    pub name: String,
    pub host_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub host_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateGroupRequest {
    pub id: i64,
    pub host_ids: Vec<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    Text,
    Number,
    Choice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptInput {
    pub name: String,
    pub label: String,
    pub kind: InputKind,
    pub required: bool,
    pub default_value: Option<String>,
    pub per_machine: bool,
    pub choices: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub id: i64,
    pub name: String,
    pub body: String,
    pub updated_at: i64,
    pub inputs: Vec<ScriptInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptRequest {
    pub name: String,
    pub body: String,
    pub inputs: Vec<ScriptInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateScriptRequest {
    pub id: i64,
    pub script: ScriptRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageScriptsRequest {
    pub image_id: i64,
    pub script_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootPartitionRequest {
    pub image_id: i64,
    pub partition_number: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptRun {
    pub position: i64,
    pub script_name: String,
    pub output: String,
    pub exit_code: Option<i64>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}
