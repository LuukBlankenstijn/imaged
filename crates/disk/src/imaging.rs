use std::path::Path;

use async_compression::tokio::bufread::{ZstdDecoder, ZstdEncoder};
use tokio::io::{AsyncRead, BufReader};

use crate::{Filesystem, partclone, swap};

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
            Filesystem::Swap => Self::SwapHeader,
        }
    }

    fn capture(&self, source: &Path, logfile: &Path) -> anyhow::Result<Captured> {
        match self {
            Self::Partclone(binary) => partclone::capture(binary, source, logfile),
            Self::SwapHeader => swap::capture(source),
        }
    }

    async fn restore(
        &self,
        data: impl AsyncRead + Unpin,
        target: &Path,
        logfile: &Path,
    ) -> anyhow::Result<()> {
        match self {
            Self::Partclone(binary) => partclone::restore(binary, data, target, logfile).await,
            Self::SwapHeader => swap::restore(data, target).await,
        }
    }
}

pub fn clone_compressed(
    filesystem: Filesystem,
    source: &Path,
    logfile: &Path,
) -> anyhow::Result<Compressed> {
    let captured = Imager::of(filesystem).capture(source, logfile)?;
    Ok(ZstdEncoder::new(BufReader::new(captured)))
}

pub async fn restore_compressed(
    filesystem: Filesystem,
    compressed: impl AsyncRead + Unpin,
    target: &Path,
    logfile: &Path,
) -> anyhow::Result<()> {
    let data = ZstdDecoder::new(BufReader::new(compressed));
    Imager::of(filesystem).restore(data, target, logfile).await
}
