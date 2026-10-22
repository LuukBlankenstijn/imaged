use std::fmt;
use std::io::{self, SeekFrom};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::Path;

use anyhow::{Context as _, ensure};
use tokio::fs::File;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use crate::partition_table::Partition;

const FILE_SECTOR_SIZE: u32 = 512;
const MBR_BYTES: usize = 512;
const MBR_ENTRIES: std::ops::Range<usize> = 446..510;
const MBR_ENTRY_BYTES: usize = 16;
const MBR_TYPE_OFFSET: usize = 4;
const PROTECTIVE_MBR_TYPE: u8 = 0xEE;
const BOOT_SIGNATURE: [u8; 2] = [0x55, 0xAA];
const SIGNATURE: [u8; 8] = *b"EFI PART";
const BACKUP_BLOCK_BYTES: usize = 512;
const BACKUP_HEADER_OFFSET: usize = BACKUP_BLOCK_BYTES;
const BACKUP_ENTRIES_OFFSET: usize = 3 * BACKUP_BLOCK_BYTES;
const HEADER_BYTES: usize = 92;
const HEADER_ENTRY_COUNT: usize = 80;
const HEADER_ENTRY_BYTES: usize = 84;
const HEADER_SECTORS: u64 = 1;
const MIN_ENTRY_BYTES: usize = 128;
const ENTRY_TYPE: std::ops::Range<usize> = 0..16;
const ENTRY_FIRST_LBA: usize = 32;
const ENTRY_LAST_LBA: usize = 40;
const UNUSED_TYPE: [u8; 16] = [0; 16];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Gpt,
    Dos,
    ProtectiveMbrOnly,
    Missing,
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Gpt => "gpt",
            Self::Dos => "dos",
            Self::ProtectiveMbrOnly => "a protective MBR without a GPT header",
            Self::Missing => "no partition table",
        })
    }
}

pub struct Disk {
    file: File,
    pub block_device: bool,
    pub sector_size: u32,
    pub size_bytes: u64,
}

impl Disk {
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let failed = || format!("inspecting {}", path.display());
        let mut file = File::open(path).await.with_context(failed)?;
        let metadata = file.metadata().await.with_context(failed)?;
        let block_device = metadata.file_type().is_block_device();
        let sector_size = if block_device {
            logical_sector_size(metadata.rdev()).await?
        } else {
            FILE_SECTOR_SIZE
        };
        let size_bytes = file.seek(SeekFrom::End(0)).await.with_context(failed)?;
        Ok(Self {
            file,
            block_device,
            sector_size,
            size_bytes,
        })
    }

    pub async fn label(&mut self) -> io::Result<Label> {
        let mut mbr = [0; MBR_BYTES];
        if !self.read_at(0, &mut mbr).await? || mbr[MBR_BYTES - 2..] != BOOT_SIGNATURE {
            return Ok(Label::Missing);
        }
        let protective = mbr[MBR_ENTRIES]
            .chunks_exact(MBR_ENTRY_BYTES)
            .any(|entry| entry[MBR_TYPE_OFFSET] == PROTECTIVE_MBR_TYPE);
        if !protective {
            return Ok(Label::Dos);
        }
        let mut signature = [0; SIGNATURE.len()];
        let header = u64::from(self.sector_size);
        Ok(
            if self.read_at(header, &mut signature).await? && signature == SIGNATURE {
                Label::Gpt
            } else {
                Label::ProtectiveMbrOnly
            },
        )
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<bool> {
        if offset.saturating_add(buf.len() as u64) > self.size_bytes {
            return Ok(false);
        }
        self.file.seek(SeekFrom::Start(offset)).await?;
        self.file.read_exact(buf).await?;
        Ok(true)
    }
}

async fn logical_sector_size(device: u64) -> anyhow::Result<u32> {
    let major = ((device >> 8) & 0xfff) | ((device >> 32) & 0xffff_f000);
    let minor = (device & 0xff) | ((device >> 12) & 0xffff_ff00);
    let path = format!("/sys/dev/block/{major}:{minor}/queue/logical_block_size");
    tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("reading {path}"))?
        .trim()
        .parse()
        .with_context(|| format!("parsing {path}"))
}

#[derive(Debug)]
pub struct Backup {
    entry_array_bytes: u64,
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    number: i64,
    first_lba: u64,
    last_lba: u64,
}

impl Backup {
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        let header = bytes
            .get(BACKUP_HEADER_OFFSET..BACKUP_HEADER_OFFSET + HEADER_BYTES)
            .filter(|header| header.starts_with(&SIGNATURE))
            .context("the partition table backup holds no GPT header")?;
        let count = le_u32(header, HEADER_ENTRY_COUNT) as usize;
        let entry_bytes = le_u32(header, HEADER_ENTRY_BYTES) as usize;
        ensure!(
            entry_bytes >= MIN_ENTRY_BYTES,
            "the partition table backup has {entry_bytes}-byte partition entries"
        );
        let array = count
            .checked_mul(entry_bytes)
            .and_then(|len| bytes.get(BACKUP_ENTRIES_OFFSET..)?.get(..len))
            .context("the partition table backup ends inside its partition entries")?;
        let entries = array
            .chunks_exact(entry_bytes)
            .zip(1..)
            .filter(|(entry, _)| entry[ENTRY_TYPE] != UNUSED_TYPE)
            .map(|(entry, number)| {
                let entry = Entry {
                    number,
                    first_lba: le_u64(entry, ENTRY_FIRST_LBA),
                    last_lba: le_u64(entry, ENTRY_LAST_LBA),
                };
                ensure!(
                    entry.first_lba <= entry.last_lba,
                    "partition {number} ends before it starts"
                );
                Ok(entry)
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Self {
            entry_array_bytes: array.len() as u64,
            entries,
        })
    }

    pub fn required_sectors(&self, sector_size: u32) -> u64 {
        let entry_array_sectors = self.entry_array_bytes.div_ceil(u64::from(sector_size));
        self.entries
            .iter()
            .map(|entry| entry.last_lba)
            .max()
            .map_or(0, |last_lba| {
                last_lba
                    .saturating_add(1)
                    .saturating_add(entry_array_sectors)
                    .saturating_add(HEADER_SECTORS)
            })
    }

    pub fn partitions(&self, sector_size: u32) -> anyhow::Result<Vec<Partition>> {
        let sector_size = u64::from(sector_size);
        self.entries
            .iter()
            .map(|entry| {
                let end = entry
                    .last_lba
                    .checked_add(1)
                    .and_then(|sectors| sectors.checked_mul(sector_size))
                    .with_context(|| {
                        format!("partition {} ends beyond 2^64 bytes", entry.number)
                    })?;
                let offset = entry.first_lba * sector_size;
                Ok(Partition {
                    number: entry.number,
                    offset,
                    size_bytes: end - offset,
                })
            })
            .collect()
    }
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    let mut field = [0; 4];
    field.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(field)
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    let mut field = [0; 8];
    field.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup(entries: &[(usize, u64, u64)]) -> Vec<u8> {
        let mut bytes = vec![0; BACKUP_ENTRIES_OFFSET + 128 * 128];
        let header = &mut bytes[BACKUP_HEADER_OFFSET..];
        header[..8].copy_from_slice(&SIGNATURE);
        header[80..84].copy_from_slice(&128u32.to_le_bytes());
        header[84..88].copy_from_slice(&128u32.to_le_bytes());
        for &(index, first_lba, last_lba) in entries {
            let entry = &mut bytes[BACKUP_ENTRIES_OFFSET + index * 128..];
            entry[..16].fill(0xAB);
            entry[32..40].copy_from_slice(&first_lba.to_le_bytes());
            entry[40..48].copy_from_slice(&last_lba.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn partitions_are_numbered_by_entry_and_sized_in_the_given_sectors() {
        let backup = Backup::parse(&backup(&[(0, 256, 511), (2, 512, 1279)])).unwrap();

        assert_eq!(
            backup.partitions(4096).unwrap(),
            [
                Partition {
                    number: 1,
                    offset: 1024 * 1024,
                    size_bytes: 1024 * 1024,
                },
                Partition {
                    number: 3,
                    offset: 2 * 1024 * 1024,
                    size_bytes: 3 * 1024 * 1024,
                },
            ]
        );
    }

    #[test]
    fn the_required_disk_holds_the_last_partition_and_the_secondary_gpt_in_the_given_sectors() {
        let backup = Backup::parse(&backup(&[(0, 256, 1279)])).unwrap();

        assert_eq!(backup.required_sectors(4096), 1279 + 4 + 2);
        assert_eq!(backup.required_sectors(512), 1279 + 32 + 2);
    }

    #[test]
    fn a_partition_that_ends_before_it_starts_is_rejected() {
        let error = Backup::parse(&backup(&[(1, 2048, 2047)])).unwrap_err();

        assert_eq!(error.to_string(), "partition 2 ends before it starts");
    }
}
