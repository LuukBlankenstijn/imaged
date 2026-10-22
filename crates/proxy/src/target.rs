use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

const THIS_NETWORK: (Ipv4Addr, u32) = (Ipv4Addr::UNSPECIFIED, 8);
const SHARED_ADDRESS_SPACE: (Ipv4Addr, u32) = (Ipv4Addr::new(100, 64, 0, 0), 10);
const PROTOCOL_ASSIGNMENTS: (Ipv4Addr, u32) = (Ipv4Addr::new(192, 0, 0, 0), 24);
const BENCHMARKING: (Ipv4Addr, u32) = (Ipv4Addr::new(198, 18, 0, 0), 15);
const RESERVED: (Ipv4Addr, u32) = (Ipv4Addr::new(240, 0, 0, 0), 4);

const NAT64_WELL_KNOWN: (Ipv6Addr, u32) = (Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96);
const NAT64_LOCAL_USE: (Ipv6Addr, u32) = (Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48);
const IPV4_COMPATIBLE: (Ipv6Addr, u32) = (Ipv6Addr::UNSPECIFIED, 96);
const SIX_TO_FOUR: (Ipv6Addr, u32) = (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16);
const SITE_LOCAL: (Ipv6Addr, u32) = (Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10);

pub(crate) fn canonical(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => embedded_ipv4(v6).map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let bits = u128::from(ip);
    if in_v6_network(ip, NAT64_WELL_KNOWN) || in_v6_network(ip, IPV4_COMPATIBLE) {
        Some(Ipv4Addr::from(bits as u32))
    } else if in_v6_network(ip, SIX_TO_FOUR) {
        Some(Ipv4Addr::from((bits >> 80) as u32))
    } else {
        None
    }
}

pub(crate) fn is_public(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(ip) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_multicast()
                || [
                    THIS_NETWORK,
                    SHARED_ADDRESS_SPACE,
                    PROTOCOL_ASSIGNMENTS,
                    BENCHMARKING,
                    RESERVED,
                ]
                .into_iter()
                .any(|network| in_v4_network(ip, network)))
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unicast_link_local()
                || ip.is_unique_local()
                || ip.is_multicast()
                || in_v6_network(ip, SITE_LOCAL)
                || in_v6_network(ip, NAT64_LOCAL_USE))
        }
    }
}

fn in_v4_network(ip: Ipv4Addr, (network, prefix_len): (Ipv4Addr, u32)) -> bool {
    let mask = u32::MAX << (32 - prefix_len);
    u32::from(ip) & mask == u32::from(network)
}

fn in_v6_network(ip: Ipv6Addr, (network, prefix_len): (Ipv6Addr, u32)) -> bool {
    let mask = u128::MAX << (128 - prefix_len);
    u128::from(ip) & mask == u128::from(network)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(address: &str) -> IpAddr {
        address.parse().unwrap()
    }

    #[test]
    fn ipv6_forms_embedding_a_private_ipv4_address_are_not_public() {
        for address in [
            "64:ff9b::a01:203",
            "64:ff9b::7f00:1",
            "::10.1.2.3",
            "::127.0.0.1",
            "2002:c0a8:101::1",
            "2002:7f00:1::",
            "::ffff:192.168.1.1",
        ] {
            assert!(!is_public(ip(address)), "{address}");
        }
    }

    #[test]
    fn ipv6_forms_embedding_a_public_ipv4_address_stay_public() {
        for address in ["64:ff9b::808:808", "2002:808:808::1", "::ffff:8.8.8.8"] {
            assert!(is_public(ip(address)), "{address}");
        }
    }

    #[test]
    fn reserved_and_site_local_ranges_are_not_public() {
        for address in [
            "192.0.0.8",
            "198.18.0.1",
            "198.19.255.255",
            "240.0.0.1",
            "255.255.255.255",
            "fec0::1",
            "64:ff9b:1::a",
        ] {
            assert!(!is_public(ip(address)), "{address}");
        }
    }

    #[test]
    fn embedded_ipv4_addresses_canonicalise_to_the_ipv4_address() {
        for address in [
            "64:ff9b::808:808",
            "::8.8.8.8",
            "2002:808:808::1",
            "::ffff:8.8.8.8",
        ] {
            assert_eq!(canonical(ip(address)), ip("8.8.8.8"), "{address}");
        }
        assert_eq!(canonical(ip("2001:db8::1")), ip("2001:db8::1"));
    }
}
