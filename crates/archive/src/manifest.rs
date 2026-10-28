use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use imaged_model::error::{AppError, Result};
use imaged_model::image::ImageKind;
use imaged_model::script::{ScriptInput, validate_script_name};
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 3;
pub const COMPATIBLE_FORMAT: u32 = 2;
const SECTOR_SIZES: std::ops::RangeInclusive<u32> = 512..=65536;
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
    #[serde(default, skip_serializing_if = "ImageKind::is_disk")]
    pub kind: ImageKind,
    pub captured_at: Option<DateTime<Utc>>,
    pub sector_size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_table: Option<ArchivedFile>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_partition: Option<i64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_label: Option<String>,
}

impl Manifest {
    pub fn parse(json: &[u8]) -> Result<Self> {
        if json.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(AppError::InvalidArgument(format!(
                "{MANIFEST_FILE} exceeds {MAX_MANIFEST_BYTES} bytes"
            )));
        }
        let probe: FormatProbe = serde_json::from_slice(json).map_err(invalid_manifest)?;
        if !(COMPATIBLE_FORMAT..=FORMAT).contains(&probe.format) {
            return Err(AppError::InvalidArgument(format!(
                "unsupported archive format {}, expected {COMPATIBLE_FORMAT} or {FORMAT}",
                probe.format
            )));
        }
        serde_json::from_slice(json).map_err(invalid_manifest)
    }

    pub fn required_format(&self) -> u32 {
        if self.kind.is_partition() || self.scripts.iter().any(|s| s.root_partition.is_some()) {
            FORMAT
        } else {
            COMPATIBLE_FORMAT
        }
    }

    pub fn checksums(&self) -> Result<BTreeMap<&str, &str>> {
        if self.format < self.required_format() {
            return Err(AppError::InvalidArgument(format!(
                "archive format {} cannot hold partition images or per-script root partitions, expected format {FORMAT}",
                self.format
            )));
        }
        if !self.sector_size.is_power_of_two() || !SECTOR_SIZES.contains(&self.sector_size) {
            return Err(AppError::InvalidArgument(format!(
                "invalid sector size {}, expected a power of two from {} to {}",
                self.sector_size,
                SECTOR_SIZES.start(),
                SECTOR_SIZES.end()
            )));
        }
        let mut checksums = BTreeMap::new();
        match (self.kind, &self.partition_table) {
            (ImageKind::Disk, None) => {
                return Err(AppError::InvalidArgument(
                    "a disk image archive must hold a partition table".to_string(),
                ));
            }
            (ImageKind::Disk, Some(table)) if table.file != PARTITION_TABLE_FILE => {
                return Err(AppError::InvalidArgument(format!(
                    "partition table must be stored as {PARTITION_TABLE_FILE}, not `{}`",
                    table.file
                )));
            }
            (ImageKind::Disk, Some(table)) => {
                checksums.insert(PARTITION_TABLE_FILE, table.sha256.as_str());
            }
            (ImageKind::Partition, Some(_)) => {
                return Err(AppError::InvalidArgument(
                    "a partition image archive cannot hold a partition table".to_string(),
                ));
            }
            (ImageKind::Partition, None) => {}
        }
        if self.partitions.is_empty() {
            return Err(AppError::InvalidArgument(
                "the manifest lists no partitions".to_string(),
            ));
        }
        if self.kind.is_partition()
            && (self.partitions.len() != 1 || self.partitions[0].number != 1)
        {
            return Err(AppError::InvalidArgument(
                "a partition image archive must hold exactly partition 1".to_string(),
            ));
        }
        let fstypes = self.kind.fstypes();
        for partition in &self.partitions {
            if partition.number < 1 {
                return Err(AppError::InvalidArgument(format!(
                    "invalid partition number {}",
                    partition.number
                )));
            }
            if !fstypes.contains(&partition.fstype.as_str()) {
                return Err(AppError::InvalidArgument(format!(
                    "partition {} has unsupported fstype `{}`, expected one of {}",
                    partition.number,
                    partition.fstype,
                    fstypes.join(", ")
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
        let archived = |number: i64| self.partitions.iter().any(|p| p.number == number);
        if let Some(root) = self.root_partition
            && !archived(root)
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
            if let Some(root) = script.root_partition
                && !archived(root)
            {
                return Err(AppError::InvalidArgument(format!(
                    "script `{}` root partition {root} is not one of the archived partitions",
                    script.name
                )));
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
