use std::{
    net::SocketAddr,
    path::{Component, Path},
};

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
        let relative: std::path::PathBuf = path
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .collect();
        tracing::debug!(path = %relative.display(), "tftp read request");

        let bytes = relative
            .to_str()
            .and_then(boot_file)
            .ok_or(packet::Error::FileNotFound)?;
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

pub async fn serve(bind_address: SocketAddr) -> std::io::Result<()> {
    tracing::info!(%bind_address, "starting tftp server");
    TftpServerBuilder::with_handler(BootFiles)
        .bind(bind_address)
        .build()
        .await
        .map_err(std::io::Error::other)?
        .serve()
        .await
        .map_err(std::io::Error::other)
}
