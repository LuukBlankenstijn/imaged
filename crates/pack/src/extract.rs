use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::Context as _;

use crate::layout::Partition;

const CHUNK_BYTES: u64 = 1024 * 1024;

pub async fn extract(disk: &Path, partition: Partition, target: &Path) -> anyhow::Result<()> {
    let disk = disk.to_path_buf();
    let target = target.to_path_buf();
    tokio::task::spawn_blocking(move || copy_sparse(&disk, partition, &target)).await?
}

fn copy_sparse(disk: &Path, partition: Partition, target: &Path) -> anyhow::Result<()> {
    let mut source = File::open(disk).with_context(|| format!("opening {}", disk.display()))?;
    source.seek(SeekFrom::Start(partition.offset))?;
    let mut output =
        File::create(target).with_context(|| format!("creating {}", target.display()))?;
    let mut chunk = vec![0; CHUNK_BYTES as usize];
    let mut remaining = partition.size_bytes;
    while remaining > 0 {
        let len = remaining.min(CHUNK_BYTES);
        let data = &mut chunk[..len as usize];
        source.read_exact(data).with_context(|| {
            format!(
                "{} ends before partition {} does",
                disk.display(),
                partition.number
            )
        })?;
        if data.iter().all(|&byte| byte == 0) {
            output.seek(SeekFrom::Current(len as i64))?;
        } else {
            output
                .write_all(data)
                .with_context(|| write_failed(target))?;
        }
        remaining -= len;
    }
    output
        .set_len(partition.size_bytes)
        .with_context(|| write_failed(target))
}

fn write_failed(target: &Path) -> String {
    format!("writing {}", target.display())
}
