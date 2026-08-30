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
    #[should_panic(expected = "overflow")]
    fn a_slot_large_enough_to_overflow_the_u16_port_panics() {
        let _ = get_multicast_port(8_000);
    }

    #[test]
    fn a_slot_that_wraps_the_u16_cast_collides_with_the_partition_table_but_is_unreachable_from_real_partition_numbers()
     {
        assert_eq!(get_multicast_port(32_768), get_multicast_port(0));
    }
}
