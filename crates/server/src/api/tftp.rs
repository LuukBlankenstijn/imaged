use async_tftp::Result;
use async_tftp::server::TftpServerBuilder;

pub async fn serve() -> Result<()> {
    let tftpd = TftpServerBuilder::with_dir_ro("./tftp")?.build().await?;
    tftpd.serve().await?;
    Ok(())
}
