use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use imaged_model::error::{AppError, Result};
use imaged_model::script::{ScriptInput, validate_script_name};
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;
pub const MANIFEST_FILE: &str = "manifest.json";
pub const PARTITION_TABLE_FILE: &str = "parttable.bin";
pub const SCRIPTS_DIR: &str = "scripts";
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
pub const MAX_PARTITION_TABLE_BYTES: u64 = 1024 * 1024;
pub const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
struct FormatProbe {
    format: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub name: String,
    pub captured_at: Option<DateTime<Utc>>,
    pub partition_table: ArchivedFile,
    pub partitions: Vec<ArchivedPartition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_partition: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<ArchivedScript>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedScript {
    pub name: String,
    pub file: String,
    pub sha256: String,
    pub inputs: Vec<ScriptInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedFile {
    pub file: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedPartition {
    pub number: i64,
    pub fstype: String,
    pub size_bytes: u64,
    pub file: String,
    pub sha256: String,
}

impl Manifest {
    pub fn parse(json: &[u8]) -> Result<Self> {
        if json.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(AppError::InvalidArgument(format!(
                "{MANIFEST_FILE} exceeds {MAX_MANIFEST_BYTES} bytes"
            )));
        }
        let probe: FormatProbe = serde_json::from_slice(json).map_err(invalid_manifest)?;
        if probe.format != FORMAT {
            return Err(AppError::InvalidArgument(format!(
                "unsupported archive format {}, expected {FORMAT}",
                probe.format
            )));
        }
        serde_json::from_slice(json).map_err(invalid_manifest)
    }

    pub fn checksums(&self) -> Result<BTreeMap<&str, &str>> {
        if self.partition_table.file != PARTITION_TABLE_FILE {
            return Err(AppError::InvalidArgument(format!(
                "partition table must be stored as {PARTITION_TABLE_FILE}, not `{}`",
                self.partition_table.file
            )));
        }
        let mut checksums =
            BTreeMap::from([(PARTITION_TABLE_FILE, self.partition_table.sha256.as_str())]);
        for partition in &self.partitions {
            if partition.number < 1 {
                return Err(AppError::InvalidArgument(format!(
                    "invalid partition number {}",
                    partition.number
                )));
            }
            let expected_file = partition_file(partition.number);
            if partition.file != expected_file {
                return Err(AppError::InvalidArgument(format!(
                    "partition {} must be stored as {expected_file}, not `{}`",
                    partition.number, partition.file
                )));
            }
            if checksums
                .insert(&partition.file, &partition.sha256)
                .is_some()
            {
                return Err(AppError::InvalidArgument(format!(
                    "partition {} is listed twice",
                    partition.number
                )));
            }
        }
        if let Some(root) = self.root_partition
            && !self.partitions.iter().any(|p| p.number == root)
        {
            return Err(AppError::InvalidArgument(format!(
                "root partition {root} is not one of the archived partitions"
            )));
        }
        for script in &self.scripts {
            validate_script_name(&script.name)?;
            for input in &script.inputs {
                input.validate()?;
            }
            let expected_file = script_file(&script.name);
            if script.file != expected_file {
                return Err(AppError::InvalidArgument(format!(
                    "script `{}` must be stored as {expected_file}, not `{}`",
                    script.name, script.file
                )));
            }
            if checksums.insert(&script.file, &script.sha256).is_some() {
                return Err(AppError::InvalidArgument(format!(
                    "script `{}` is listed twice",
                    script.name
                )));
            }
        }
        Ok(checksums)
    }
}

pub fn partition_file(number: i64) -> String {
    format!("p-{number}.pcl")
}

pub fn script_file(name: &str) -> String {
    format!("{SCRIPTS_DIR}/{name}.sh")
}

pub(crate) fn is_script_file(name: &str) -> bool {
    name.strip_prefix(SCRIPTS_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

pub fn archive_file_name(image_name: &str) -> String {
    let stem: String = image_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() {
        "image.imaged".to_string()
    } else {
        format!("{stem}.imaged")
    }
}

fn invalid_manifest(error: serde_json::Error) -> AppError {
    AppError::InvalidArgument(format!("invalid {MANIFEST_FILE}: {error}"))
}
