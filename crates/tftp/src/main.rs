use std::net::SocketAddr;

use async_tftp::{
    packet::{self, Error},
    server::{Handler, TftpServerBuilder},
};
use futures_lite::io::Cursor;

fn main() -> Result<(), Error> {
    imaged_shared::setup_logging!("info");
    tracing::info!(interface = "0.0.0.0", port = 69, "starting imaged-tftp");
    futures_lite::future::block_on(async {
        TftpServerBuilder::with_handler(StaticBytesHandler {})
            .build()
            .await?
            .serve()
            .await?;
        Ok(())
    })
}

static IPXE: &[u8] = include_bytes!("../../../assets/ipxe.efi");
static UNDIONLY: &[u8] = include_bytes!("../../../assets/undionly.kpxe");

struct StaticBytesHandler {}

impl Handler for StaticBytesHandler {
    type Reader = Cursor<&'static [u8]>;
    type Writer = Cursor<Vec<u8>>;

    async fn read_req_open(
        &mut self,
        _client: &std::net::SocketAddr,
        path: &std::path::Path,
    ) -> Result<(Self::Reader, Option<u64>), async_tftp::packet::Error> {
        let req_path = path
            .strip_prefix("/")
            .or_else(|_| path.strip_prefix("./"))
            .unwrap_or(path)
            .to_owned();

        tracing::debug!(path=%&req_path.to_string_lossy(), "handling request");

        match req_path.to_str() {
            Some("ipxe.efi") => Ok((Cursor::new(IPXE), None)),
            Some("undionly.kpxe") => Ok((Cursor::new(UNDIONLY), None)),
            _ => Err(packet::Error::FileNotFound),
        }
    }

    async fn write_req_open(
        &mut self,
        _client: &SocketAddr,
        _path: &std::path::Path,
        _size: Option<u64>,
    ) -> Result<Self::Writer, packet::Error> {
        Err(packet::Error::IllegalOperation)
    }
}
