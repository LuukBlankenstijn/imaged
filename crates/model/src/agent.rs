use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::AppError;
pub use crate::image::ImagePartition;

pub const MAC_HEADER: &str = "X-Agent-Mac";
pub const IP_HEADER: &str = "X-Agent-Ip";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MacAddress([u8; 6]);

impl From<[u8; 6]> for MacAddress {
    fn from(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }
}

impl FromStr for MacAddress {
    type Err = AppError;

    fn from_str(text: &str) -> Result<Self, AppError> {
        let invalid = || AppError::InvalidArgument(format!("invalid MAC address {text:?}"));
        let separator = if text.contains('-') { '-' } else { ':' };
        let mut parts = text.split(separator);
        let mut bytes = [0; 6];
        for byte in &mut bytes {
            let part = parts
                .next()
                .filter(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(invalid)?;
            *byte = u8::from_str_radix(part, 16).map_err(|_| invalid())?;
        }
        match parts.next() {
            Some(_) => Err(invalid()),
            None => Ok(Self(bytes)),
        }
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}

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
    pub disk_size_bytes: Option<u64>,
    pub disk_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionTableUpload {
    pub sector_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageLayout {
    pub sector_size: u32,
    pub partitions: Vec<ImagePartition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionUpload {
    pub filesystem: String,
    pub size: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptOutputUpload {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFailure {
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptFinished {
    pub exit_code: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(text: &str) -> String {
        text.parse::<MacAddress>().unwrap().to_string()
    }

    #[test]
    fn mac_addresses_are_canonicalised_to_lowercase_colon_form() {
        assert_eq!(canonical("AA:BB:CC:DD:EE:0F"), "aa:bb:cc:dd:ee:0f");
        assert_eq!(canonical("aa-bb-cc-dd-ee-0f"), "aa:bb:cc:dd:ee:0f");
        assert_eq!(canonical("aa:bb:cc:dd:ee:0f"), "aa:bb:cc:dd:ee:0f");
        assert_eq!(
            MacAddress::from([0xde, 0xad, 0xbe, 0xef, 0, 1]).to_string(),
            "de:ad:be:ef:00:01"
        );
    }

    #[test]
    fn malformed_mac_addresses_are_rejected() {
        for text in [
            "",
            "foo",
            "aa:bb:cc:dd:ee",
            "aa:bb:cc:dd:ee:ff:00",
            "aa:bb:cc:dd:ee:f",
            "aa:bb:cc:dd:ee:fff",
            "aa:bb:cc:dd:ee:gg",
            "aa:bb:cc:dd:ee:+f",
            "aa:bb-cc:dd:ee:ff",
            "aabbccddeeff",
            " aa:bb:cc:dd:ee:ff",
        ] {
            assert!(
                matches!(
                    text.parse::<MacAddress>(),
                    Err(AppError::InvalidArgument(_))
                ),
                "{text:?} must be rejected"
            );
        }
    }
}
