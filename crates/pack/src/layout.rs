use std::path::Path;

use anyhow::{Context as _, bail};
use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub number: i64,
    pub offset: u64,
    pub size_bytes: u64,
}

#[derive(Deserialize)]
struct Dump {
    partitiontable: Table,
}

#[derive(Deserialize)]
struct Table {
    label: String,
    #[serde(default = "default_sector_size")]
    sectorsize: u64,
    #[serde(default)]
    partitions: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    node: String,
    start: u64,
    size: u64,
}

fn default_sector_size() -> u64 {
    512
}

pub async fn partitions(disk: &Path) -> anyhow::Result<Vec<Partition>> {
    let output = Command::new("sfdisk")
        .arg("--json")
        .arg(disk)
        .kill_on_drop(true)
        .output()
        .await
        .context("running sfdisk")?;
    if !output.status.success() {
        bail!(
            "sfdisk could not read the partition table of {}: {}",
            disk.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse(&disk.to_string_lossy(), &output.stdout)
}

fn parse(disk: &str, json: &[u8]) -> anyhow::Result<Vec<Partition>> {
    let table = serde_json::from_slice::<Dump>(json)
        .context("parsing sfdisk output")?
        .partitiontable;
    if table.label != "gpt" {
        bail!("only GPT disks are supported, found {}", table.label);
    }
    table
        .partitions
        .iter()
        .map(|entry| {
            Ok(Partition {
                number: partition_number(disk, &entry.node)?,
                offset: entry.start * table.sectorsize,
                size_bytes: entry.size * table.sectorsize,
            })
        })
        .collect()
}

fn partition_number(disk: &str, node: &str) -> anyhow::Result<i64> {
    node.strip_prefix(disk)
        .map(|suffix| suffix.strip_prefix('p').unwrap_or(suffix))
        .and_then(|number| number.parse().ok())
        .with_context(|| format!("cannot tell the partition number of {node}"))
}

pub async fn fstype(source: &Path) -> anyhow::Result<Option<String>> {
    let output = Command::new("blkid")
        .args(["-p", "-o", "value", "-s", "TYPE"])
        .arg(source)
        .kill_on_drop(true)
        .output()
        .await
        .context("running blkid")?;
    match output.status.code() {
        Some(0) => {
            let fstype = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            Ok((!fstype.is_empty()).then_some(fstype))
        }
        Some(2) => Ok(None),
        _ => bail!(
            "blkid could not probe {}: {}",
            source.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(disk: &str, label: &str, sectorsize: Option<u64>, nodes: &[&str]) -> Vec<u8> {
        let mut table = serde_json::json!({
            "label": label,
            "partitions": nodes
                .iter()
                .enumerate()
                .map(|(i, node)| serde_json::json!({
                    "node": node,
                    "start": 2048 * (i as u64 + 1),
                    "size": 1024,
                    "type": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
                }))
                .collect::<Vec<_>>(),
            "device": disk,
        });
        if let Some(sectorsize) = sectorsize {
            table["sectorsize"] = sectorsize.into();
        }
        serde_json::to_vec(&serde_json::json!({ "partitiontable": table })).unwrap()
    }

    #[test]
    fn sectors_are_converted_to_bytes_with_the_table_sector_size() {
        let json = dump("disk.img", "gpt", Some(4096), &["disk.img1", "disk.img3"]);
        assert_eq!(
            parse("disk.img", &json).unwrap(),
            [
                Partition {
                    number: 1,
                    offset: 2048 * 4096,
                    size_bytes: 1024 * 4096,
                },
                Partition {
                    number: 3,
                    offset: 4096 * 4096,
                    size_bytes: 1024 * 4096,
                },
            ]
        );
    }

    #[test]
    fn a_missing_sector_size_means_512_bytes() {
        let json = dump("disk.img", "gpt", None, &["disk.img2"]);
        assert_eq!(
            parse("disk.img", &json).unwrap(),
            [Partition {
                number: 2,
                offset: 2048 * 512,
                size_bytes: 1024 * 512,
            }]
        );
    }

    #[test]
    fn disks_whose_path_ends_in_a_digit_have_p_separated_nodes() {
        let json = dump(
            "images/disk0",
            "gpt",
            None,
            &["images/disk0p1", "images/disk0p12"],
        );
        let numbers: Vec<i64> = parse("images/disk0", &json)
            .unwrap()
            .iter()
            .map(|p| p.number)
            .collect();
        assert_eq!(numbers, [1, 12]);
    }

    #[test]
    fn a_node_that_does_not_belong_to_the_disk_is_an_error() {
        let json = dump("disk.img", "gpt", None, &["other.img1"]);
        assert_eq!(
            parse("disk.img", &json).unwrap_err().to_string(),
            "cannot tell the partition number of other.img1"
        );
    }

    #[test]
    fn only_gpt_disks_are_supported() {
        let json = dump("disk.img", "dos", None, &["disk.img1"]);
        assert_eq!(
            parse("disk.img", &json).unwrap_err().to_string(),
            "only GPT disks are supported, found dos"
        );
    }
}
