#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mac address: {0}")]
    MacAddress(#[from] mac_address::MacAddressError),
    #[error("no mac address found")]
    NoMac,
    #[error("no suitable disk found")]
    NoDisk,
    #[error("multiple disks found: {0:?}")]
    MultipleDisks(Vec<String>),
    #[error("refusing to image disk {0}: contains the running root filesystem")]
    WouldImageRoot(String),
    #[error("lsblk failed: {0}")]
    Lsblk(String),
    #[error("Could not find partition number {0}")]
    NoPartitionNumber(String),
}
