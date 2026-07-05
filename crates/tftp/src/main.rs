use clap::Parser;
use std::net::SocketAddr;

use async_tftp::{
    packet::{self, Error},
    server::{Handler, TftpServerBuilder},
};
use futures_lite::io::Cursor;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// address to bind to, also used as the wake on lan interface
    #[arg(short, long, default_value_t = SocketAddr::from(([0,0,0,0], 8080)))]
    bind_address: SocketAddr,
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

fn main() -> Result<(), Error> {
    let args = Args::parse();
    imaged_shared::setup_logging!(args.log_level);
    tracing::info!(
        interface = args.bind_address.to_string(),
        "starting imaged-tftp"
    );
    futures_lite::future::block_on(async {
        TftpServerBuilder::with_handler(StaticBytesHandler {})
            .bind(args.bind_address)
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
