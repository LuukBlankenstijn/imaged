use std::{
    future::Future,
    io,
    net::SocketAddr,
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_tftp::{
    packet,
    server::{Handler, TftpServerBuilder},
};
use axum::body::Bytes;
use futures_lite::{AsyncRead, io::Cursor};

use crate::{BootClient, BootFiles, pxe::Arch};

const RETRANSMIT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RETRANSMITS: u32 = 10;

static IPXE: &[u8] = include_bytes!(concat!(env!("IMAGED_BOOT_ASSETS_DIR"), "/ipxe.efi"));
static IPXE_ARM64: &[u8] =
    include_bytes!(concat!(env!("IMAGED_BOOT_ASSETS_DIR"), "/ipxe-arm64.efi"));
static UNDIONLY: &[u8] = include_bytes!(concat!(env!("IMAGED_BOOT_ASSETS_DIR"), "/undionly.kpxe"));

fn boot_file(files: &BootFiles, client: &SocketAddr, path: &str) -> Option<(String, Vec<Bytes>)> {
    let fixed = |bytes| Some((path.to_string(), vec![Bytes::from_static(bytes)]));
    match path {
        "ipxe.efi" => return fixed(IPXE),
        "ipxe-arm64.efi" => return fixed(IPXE_ARM64),
        "undionly.kpxe" => return fixed(UNDIONLY),
        _ => {}
    }
    let ((name, profile), file) = match path.strip_prefix("profile/") {
        Some(rest) => {
            let (name, file) = rest.split_once('/')?;
            (files.named(name)?, file)
        }
        None => {
            let (arch, file) = path.split_once('/')?;
            let client = BootClient::new(arch.parse::<Arch>().ok()?, client.ip());
            (files.select(&client), file)
        }
    };
    let parts = match file {
        "vmlinuz" => vec![profile.kernel.clone()],
        "initramfs.cpio.xz" => profile.initramfs.clone(),
        _ => return None,
    };
    Some((format!("profile/{name}/{file}"), parts))
}

struct TftpFiles(Arc<BootFiles>);

struct Transfer {
    parts: std::vec::IntoIter<Bytes>,
    current: Bytes,
    size: u64,
    sent: u64,
    path: String,
    client: SocketAddr,
    started: Instant,
}

impl AsyncRead for Transfer {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        while self.current.is_empty() {
            match self.parts.next() {
                Some(part) => self.current = part,
                None => return Poll::Ready(Ok(0)),
            }
        }
        let len = buf.len().min(self.current.len());
        buf[..len].copy_from_slice(&self.current[..len]);
        self.current = self.current.slice(len..);
        self.sent += len as u64;
        Poll::Ready(Ok(len))
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        if self.sent == self.size {
            let elapsed_ms = self.started.elapsed().as_millis();
            tracing::info!(path = %self.path, client = %self.client, size = self.size, elapsed_ms, "tftp transfer done");
        }
    }
}

impl Handler for TftpFiles {
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

        let Some((served, parts)) = boot_file(&self.0, client, &relative) else {
            tracing::warn!(path = %relative, %client, "tftp request for an unknown file");
            return Err(packet::Error::FileNotFound);
        };
        let size: u64 = parts.iter().map(|part| part.len() as u64).sum();
        tracing::info!(path = %relative, %served, %client, size, "tftp transfer started");
        let transfer = Transfer {
            parts: parts.into_iter(),
            current: Bytes::new(),
            size,
            sent: 0,
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

pub struct TftpServer(async_tftp::server::TftpServer<TftpFiles>);

pub async fn bind(bind_address: SocketAddr, files: Arc<BootFiles>) -> io::Result<TftpServer> {
    let server = TftpServerBuilder::with_handler(TftpFiles(files))
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
    use crate::{
        Sources,
        files::tests::{CLIENT_IP, Fixed, TempFile, name, profile_with},
    };
    use futures_lite::AsyncReadExt;

    async fn files(sources: &Sources) -> Arc<BootFiles> {
        Arc::new(BootFiles::load(sources, None).await.unwrap())
    }

    async fn read_in_blocks(mut transfer: Transfer) -> Vec<u8> {
        let mut received = Vec::new();
        let mut block = [0; 512];
        loop {
            let len = transfer.read(&mut block).await.unwrap();
            if len == 0 {
                return received;
            }
            received.extend_from_slice(&block[..len]);
        }
    }

    async fn fetch(handler: &mut TftpFiles, path: &str) -> Option<Vec<u8>> {
        let client = SocketAddr::new(CLIENT_IP, 2000);
        let (served, size) = handler.read_req_open(&client, Path::new(path)).await.ok()?;
        let received = read_in_blocks(served).await;
        assert_eq!(size, Some(received.len() as u64), "{path}");
        Some(received)
    }

    #[tokio::test]
    async fn nested_boot_files_resolve_with_either_path_separator() {
        let mut handler = TftpFiles(files(&Sources::default()).await);
        let kernel = handler.0.profile("x86_64").unwrap().kernel.clone();
        for requested in [
            "x86_64/vmlinuz",
            "/x86_64/vmlinuz",
            "x86_64\\vmlinuz",
            "\\x86_64\\vmlinuz",
        ] {
            assert!(
                fetch(&mut handler, requested).await.unwrap() == kernel,
                "{requested}"
            );
        }
    }

    #[tokio::test]
    async fn an_initramfs_in_several_parts_is_sent_whole_with_its_total_size() {
        let extra = TempFile::new("tftp-extra-initrd", &[7; 700]);
        let mut sources = Sources::default();
        sources
            .profiles
            .insert(name("arm64"), profile_with(Arch::Arm64, None, &[&extra]));
        let mut handler = TftpFiles(files(&sources).await);
        let expected = handler.0.profile("arm64").unwrap().initramfs.concat();

        let received = fetch(&mut handler, "arm64/initramfs.cpio.xz")
            .await
            .unwrap();

        assert!(received.ends_with(&[7; 700]));
        assert!(received == expected);
    }

    #[tokio::test]
    async fn fixed_arch_names_go_through_the_selector_and_profile_names_are_served_directly() {
        let board = TempFile::new("tftp-board-initrd", b"orangepi");
        let mut sources = Sources::default();
        sources.profiles.insert(
            name("orange-pi"),
            profile_with(Arch::Arm64, None, &[&board]),
        );
        let files = Arc::new(
            BootFiles::load(&sources, Some(Arc::new(Fixed(Some("orange-pi")))))
                .await
                .unwrap(),
        );
        let mut handler = TftpFiles(files.clone());
        let board_initramfs = files.profile("orange-pi").unwrap().initramfs.concat();

        assert!(
            fetch(&mut handler, "arm64/initramfs.cpio.xz")
                .await
                .unwrap()
                == board_initramfs
        );
        assert!(
            fetch(&mut handler, "profile/orange-pi/initramfs.cpio.xz")
                .await
                .unwrap()
                == board_initramfs
        );
        assert!(
            fetch(&mut handler, "x86_64/vmlinuz").await.unwrap()
                == files.profile("x86_64").unwrap().kernel
        );
        for missing in [
            "unknown/vmlinuz",
            "orange-pi/vmlinuz",
            "profile/unknown/vmlinuz",
            "profile/orange-pi/ipxe.efi",
            "profile/orange-pi",
        ] {
            assert!(fetch(&mut handler, missing).await.is_none(), "{missing}");
        }
    }

    #[tokio::test]
    async fn an_occupied_address_reports_addr_in_use() {
        let occupied = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let error = bind(
            occupied.local_addr().unwrap(),
            files(&Sources::default()).await,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    }

    #[tokio::test]
    async fn a_client_cannot_raise_the_retransmit_timeout() {
        let files = files(&Sources::default()).await;
        let kernel_size = files.profile("x86_64").unwrap().kernel.len();
        let server = bind(SocketAddr::from(([127, 0, 0, 1], 0)), files)
            .await
            .unwrap();
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
        assert_eq!(options[..2], ["tsize".to_string(), kernel_size.to_string()]);
        assert!(!options.contains(&"timeout".to_string()), "{options:?}");
    }
}
