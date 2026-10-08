use std::{future::Future, io, net::SocketAddr, path::Path};

use async_tftp::{
    packet,
    server::{Handler, TftpServerBuilder},
};
use futures_lite::io::Cursor;

use crate::pxe::Arch;

static IPXE: &[u8] = include_bytes!("../../../assets/ipxe.efi");
static IPXE_ARM64: &[u8] = include_bytes!("../../../assets/ipxe-arm64.efi");
static UNDIONLY: &[u8] = include_bytes!("../../../assets/undionly.kpxe");

fn boot_file(path: &str) -> Option<&'static [u8]> {
    match path {
        "ipxe.efi" => Some(IPXE),
        "ipxe-arm64.efi" => Some(IPXE_ARM64),
        "undionly.kpxe" => Some(UNDIONLY),
        "x86_64/vmlinuz" => Some(Arch::X86_64.vmlinuz()),
        "x86_64/initramfs.cpio.xz" => Some(Arch::X86_64.initramfs()),
        "arm64/vmlinuz" => Some(Arch::Arm64.vmlinuz()),
        "arm64/initramfs.cpio.xz" => Some(Arch::Arm64.initramfs()),
        _ => None,
    }
}

struct BootFiles;

impl Handler for BootFiles {
    type Reader = Cursor<&'static [u8]>;
    type Writer = Cursor<Vec<u8>>;

    async fn read_req_open(
        &mut self,
        _client: &SocketAddr,
        path: &Path,
    ) -> Result<(Self::Reader, Option<u64>), packet::Error> {
        let requested = path.to_string_lossy();
        let relative = requested
            .split(['/', '\\'])
            .filter(|part| !matches!(*part, "" | "." | ".."))
            .collect::<Vec<_>>()
            .join("/");
        tracing::debug!(path = %relative, "tftp read request");

        let bytes = boot_file(&relative).ok_or(packet::Error::FileNotFound)?;
        Ok((Cursor::new(bytes), Some(bytes.len() as u64)))
    }

    async fn write_req_open(
        &mut self,
        _client: &SocketAddr,
        _path: &Path,
        _size: Option<u64>,
    ) -> Result<Self::Writer, packet::Error> {
        Err(packet::Error::IllegalOperation)
    }
}

pub struct TftpServer(async_tftp::server::TftpServer<BootFiles>);

pub async fn bind(bind_address: SocketAddr) -> io::Result<TftpServer> {
    let server = TftpServerBuilder::with_handler(BootFiles)
        .bind(bind_address)
        .build()
        .await
        .map_err(io::Error::other)?;
    tracing::info!(%bind_address, "tftp server listening");
    Ok(TftpServer(server))
}

impl TftpServer {
    pub async fn serve(self, shutdown: impl Future<Output = ()>) -> io::Result<()> {
        tokio::select! {
            served = self.0.serve() => served.map_err(io::Error::other),
            () = shutdown => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nested_boot_files_resolve_with_either_path_separator() {
        let client = SocketAddr::from(([127, 0, 0, 1], 0));
        for requested in [
            "x86_64/vmlinuz",
            "/x86_64/vmlinuz",
            "x86_64\\vmlinuz",
            "\\x86_64\\vmlinuz",
        ] {
            let (served, size) = BootFiles
                .read_req_open(&client, Path::new(requested))
                .await
                .unwrap();

            assert_eq!(served.into_inner(), Arch::X86_64.vmlinuz(), "{requested}");
            assert_eq!(size, Some(Arch::X86_64.vmlinuz().len() as u64));
        }
    }
}
