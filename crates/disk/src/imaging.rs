use std::path::Path;

use async_compression::tokio::bufread::{ZstdDecoder, ZstdEncoder};
use tokio::io::{AsyncRead, BufReader};

use crate::{Filesystem, Progress, partclone, swap};

pub type Captured = Box<dyn AsyncRead + Send + Unpin>;
pub type Compressed = ZstdEncoder<BufReader<Captured>>;

enum Imager {
    Partclone(&'static str),
    SwapHeader,
}

impl Imager {
    fn of(filesystem: Filesystem) -> Self {
        match filesystem {
            Filesystem::Ext2 | Filesystem::Ext3 | Filesystem::Ext4 => {
                Self::Partclone("partclone.extfs")
            }
            Filesystem::Vfat => Self::Partclone("partclone.vfat"),
            Filesystem::Btrfs => Self::Partclone("partclone.btrfs"),
            Filesystem::Swap => Self::SwapHeader,
        }
    }

    fn capture(
        &self,
        source: &Path,
        logfile: &Path,
        on_progress: impl Fn(Progress) + Send + 'static,
    ) -> anyhow::Result<Captured> {
        match self {
            Self::Partclone(binary) => partclone::capture(binary, source, logfile, on_progress),
            Self::SwapHeader => swap::capture(source),
        }
    }

    async fn restore(
        &self,
        data: impl AsyncRead + Unpin,
        target: &Path,
        logfile: &Path,
        on_progress: impl Fn(Progress),
    ) -> anyhow::Result<()> {
        match self {
            Self::Partclone(binary) => {
                partclone::restore(binary, data, target, logfile, on_progress).await
            }
            Self::SwapHeader => swap::restore(data, target).await,
        }
    }
}

pub fn clone_compressed(
    filesystem: Filesystem,
    source: &Path,
    logfile: &Path,
    on_progress: impl Fn(Progress) + Send + 'static,
) -> anyhow::Result<Compressed> {
    let captured = Imager::of(filesystem).capture(source, logfile, on_progress)?;
    Ok(ZstdEncoder::new(BufReader::new(captured)))
}

pub async fn restore_compressed(
    filesystem: Filesystem,
    compressed: impl AsyncRead + Unpin,
    target: &Path,
    logfile: &Path,
    on_progress: impl Fn(Progress),
) -> anyhow::Result<()> {
    let data = ZstdDecoder::new(BufReader::new(compressed));
    Imager::of(filesystem)
        .restore(data, target, logfile, on_progress)
        .await
}
