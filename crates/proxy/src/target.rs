use std::net::{IpAddr, Ipv4Addr};

const THIS_NETWORK: (Ipv4Addr, u32) = (Ipv4Addr::UNSPECIFIED, 8);
const SHARED_ADDRESS_SPACE: (Ipv4Addr, u32) = (Ipv4Addr::new(100, 64, 0, 0), 10);

pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || in_network(ip, THIS_NETWORK)
                || in_network(ip, SHARED_ADDRESS_SPACE))
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unicast_link_local()
                || ip.is_unique_local()
                || ip.is_multicast())
        }
    }
}

fn in_network(ip: Ipv4Addr, (network, prefix_len): (Ipv4Addr, u32)) -> bool {
    let mask = u32::MAX << (32 - prefix_len);
    u32::from(ip) & mask == u32::from(network)
}
