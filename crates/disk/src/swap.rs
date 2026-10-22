use std::io::{Cursor, Read as _};
use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};

use crate::imaging::Captured;

const SIGNATURE: &[u8] = b"SWAPSPACE2";
const PAGE_SIZES: [usize; 4] = [4096, 8192, 16384, 65536];
const LARGEST_PAGE: usize = PAGE_SIZES[PAGE_SIZES.len() - 1];

pub fn capture(source: &Path) -> anyhow::Result<Captured> {
    let mut start = Vec::with_capacity(LARGEST_PAGE);
    std::fs::File::open(source)
        .with_context(|| format!("opening {}", source.display()))?
        .take(LARGEST_PAGE as u64)
        .read_to_end(&mut start)
        .with_context(|| format!("reading the swap header of {}", source.display()))?;
    let page = PAGE_SIZES
        .into_iter()
        .find(|&page| start.get(page - SIGNATURE.len()..page) == Some(SIGNATURE))
        .with_context(|| format!("{} has no swap signature", source.display()))?;
    start.truncate(page);
    Ok(Box::new(Cursor::new(start)))
}

pub async fn restore(data: impl AsyncRead + Unpin, target: &Path) -> anyhow::Result<()> {
    let mut header = Vec::new();
    data.take(LARGEST_PAGE as u64 + 1)
        .read_to_end(&mut header)
        .await
        .context("reading the swap header")?;
    if !PAGE_SIZES.contains(&header.len()) || !header.ends_with(SIGNATURE) {
        bail!("the archived swap header is not a swap header page");
    }
    let mut device = tokio::fs::OpenOptions::new()
        .write(true)
        .open(target)
        .await
        .with_context(|| format!("opening {}", target.display()))?;
    device
        .write_all(&header)
        .await
        .with_context(|| format!("writing the swap header to {}", target.display()))?;
    device.sync_all().await?;
    Ok(())
}
