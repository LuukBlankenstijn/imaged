/// Rendezvous port for a single file in a multicast transfer.
///
/// Every file (partition table + each partition) gets its own port so a sender
/// never reuses the port a previous file's receiver just tore down — doing so
/// makes `udp-sender` bail immediately instead of waiting for the next
/// receiver. Only one multicast session runs at a time (the manager is
/// single-slot and the multicast addresses are fixed), so a fixed base is safe.
/// slot 0 is the partition table; slot N is partition number N.
pub fn get_multicast_port(slot: i64) -> u16 {
    50_000 + (slot * 2) as u16
}

pub const MULTICAST_RVD_ADDRESS: &str = "239.16.16.16";
pub const MULTICAST_DATA_ADDRESS: &str = "239.20.20.20";
