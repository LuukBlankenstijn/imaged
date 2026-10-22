use std::{
    future::Future,
    io,
    net::SocketAddr,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_tftp::{
    packet,
    server::{Handler, TftpServerBuilder},
};
use futures_lite::{AsyncRead, io::Cursor};

use crate::pxe::Arch;

const RETRANSMIT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RETRANSMITS: u32 = 10;

static IPXE: &[u8] = include_bytes!(concat!(env!("IMAGED_BOOT_ASSETS_DIR"), "/ipxe.efi"));
static IPXE_ARM64: &[u8] =
    include_bytes!(concat!(env!("IMAGED_BOOT_ASSETS_DIR"), "/ipxe-arm64.efi"));
static UNDIONLY: &[u8] = include_bytes!(concat!(env!("IMAGED_BOOT_ASSETS_DIR"), "/undionly.kpxe"));

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

struct Transfer {
    file: Cursor<&'static [u8]>,
    path: String,
    client: SocketAddr,
    started: Instant,
}

impl AsyncRead for Transfer {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.file).poll_read(cx, buf)
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        let size = self.file.get_ref().len() as u64;
        if self.file.position() == size {
            let elapsed_ms = self.started.elapsed().as_millis();
            tracing::info!(path = %self.path, client = %self.client, size, elapsed_ms, "tftp transfer done");
        }
    }
}

impl Handler for BootFiles {
    type Reader = Transfer;
    type Writer = Cursor<Vec<u8>>;

    async fn read_req_open(
        &mut self,
        client: &SocketAddr,
        path: &Path,
    ) -> Result<(Self::Reader, Option<u64>), packet::Error> {
        let requested = path.to_string_lossy();
        let relative = requested
            .split(['/', '\\'])
            .filter(|part| !matches!(*part, "" | "." | ".."))
            .collect::<Vec<_>>()
            .join("/");

        let Some(bytes) = boot_file(&relative) else {
            tracing::warn!(path = %relative, %client, "tftp request for an unknown file");
            return Err(packet::Error::FileNotFound);
        };
        let size = bytes.len() as u64;
        tracing::info!(path = %relative, %client, size, "tftp transfer started");
        let transfer = Transfer {
            file: Cursor::new(bytes),
            path: relative,
            client: *client,
            started: Instant::now(),
        };
        Ok((transfer, Some(size)))
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
        .timeout(RETRANSMIT_TIMEOUT)
        .ignore_client_timeout()
        .max_send_retries(MAX_RETRANSMITS)
        .build()
        .await
        .map_err(into_io_error)?;
    tracing::info!(%bind_address, "tftp server listening");
    Ok(TftpServer(server))
}

fn into_io_error(error: async_tftp::Error) -> io::Error {
    match error {
        async_tftp::Error::Io(error) | async_tftp::Error::Bind(error) => error,
        error => io::Error::other(error),
    }
}

impl TftpServer {
    pub async fn serve(self, shutdown: impl Future<Output = ()>) -> io::Result<()> {
        tokio::select! {
            served = self.0.serve() => served.map_err(into_io_error),
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

            assert_eq!(
                *served.file.get_ref(),
                Arch::X86_64.vmlinuz(),
                "{requested}"
            );
            assert_eq!(size, Some(Arch::X86_64.vmlinuz().len() as u64));
        }
    }

    #[tokio::test]
    async fn an_occupied_address_reports_addr_in_use() {
        let occupied = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let error = bind(occupied.local_addr().unwrap()).await.err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    }

    #[tokio::test]
    async fn a_client_cannot_raise_the_retransmit_timeout() {
        let server = bind(SocketAddr::from(([127, 0, 0, 1], 0))).await.unwrap();
        let address = server.0.listen_addr().unwrap();
        let serving = tokio::spawn(server.serve(std::future::pending::<()>()));
        let reply = tokio::task::spawn_blocking(move || {
            let client = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = b"\x00\x01x86_64/vmlinuz\x00octet\x00timeout\x00255\x00tsize\x000\x00";
            client.send_to(request, address).unwrap();
            let mut reply = [0; 512];
            let (len, _) = client.recv_from(&mut reply).unwrap();
            reply[..len].to_vec()
        })
        .await
        .unwrap();
        serving.abort();

        let options = reply[2..]
            .split(|&byte| byte == 0)
            .map(|field| String::from_utf8_lossy(field).into_owned())
            .collect::<Vec<_>>();
        assert_eq!(&reply[..2], b"\0\x06");
        assert_eq!(
            options[..2],
            [
                "tsize".to_string(),
                Arch::X86_64.vmlinuz().len().to_string()
            ]
        );
        assert!(!options.contains(&"timeout".to_string()), "{options:?}");
    }
}
