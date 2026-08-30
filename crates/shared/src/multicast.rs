/// Rendezvous port for a single file in a multicast transfer.
///
/// Every file (partition table + each partition) gets its own port so a sender
/// never reuses the port a previous file's receiver just tore down — doing so
/// makes `udp-sender` bail immediately instead of waiting for the next
/// receiver. Only one multicast session runs at a time (the manager is
/// single-slot and the multicast addresses are fixed), so a fixed base is safe.
/// slot 0 is the partition table; slot N is partition number N.
///
/// Valid slots `0..=MAX_MULTICAST_SLOT` map to the even ports
/// `MULTICAST_PORT_BASE..=65534`, all within the dynamic/ephemeral range.
/// Any slot outside that domain (negative, or beyond what a GPT partition
/// number can ever be) is out of contract; it returns the odd sentinel
/// `u16::MAX`, which no valid slot can produce, so a bad slot can never
/// silently alias another file's port.
pub const MULTICAST_PORT_BASE: u16 = 50_000;
pub const MAX_MULTICAST_SLOT: i64 = ((u16::MAX - MULTICAST_PORT_BASE - 1) / 2) as i64;

pub fn get_multicast_port(slot: i64) -> u16 {
    if (0..=MAX_MULTICAST_SLOT).contains(&slot) {
        MULTICAST_PORT_BASE + (slot as u16) * 2
    } else {
        u16::MAX
    }
}

pub const MULTICAST_RVD_ADDRESS: &str = "239.16.16.16";
pub const MULTICAST_DATA_ADDRESS: &str = "239.20.20.20";

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn slot_zero_is_the_partition_table_base_port() {
        assert_eq!(get_multicast_port(0), 50_000);
    }

    #[test]
    fn each_slot_maps_to_its_even_offset_from_the_base() {
        for slot in 0..=64i64 {
            assert_eq!(get_multicast_port(slot), 50_000 + (slot as u16) * 2);
        }
    }

    #[test]
    fn realistic_partition_slots_never_collide() {
        let ports: HashSet<u16> = (0..=64i64).map(get_multicast_port).collect();
        assert_eq!(ports.len(), 65);
    }

    #[test]
    fn out_of_range_slots_return_the_reserved_sentinel_port_never_a_valid_slots_port() {
        assert_eq!(get_multicast_port(MAX_MULTICAST_SLOT + 1), u16::MAX);
        assert_eq!(get_multicast_port(8_000), u16::MAX);
        assert_eq!(get_multicast_port(i64::MAX), u16::MAX);
        assert_eq!(get_multicast_port(-1), u16::MAX);
        assert_eq!(u16::MAX % 2, 1);
        assert_eq!(get_multicast_port(MAX_MULTICAST_SLOT) % 2, 0);
    }

    #[test]
    fn a_slot_that_would_wrap_the_u16_cast_no_longer_collides_with_the_partition_table() {
        assert_ne!(get_multicast_port(32_768), get_multicast_port(0));
    }
}
