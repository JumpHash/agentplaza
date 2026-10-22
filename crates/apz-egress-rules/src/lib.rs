//! The destinations an egress proxy refuses, held in one place.
//!
//! Two proxies carry traffic out for guests and agents: vsockd's, for the VMs (`crates/vsockd/src/egress.rs`),
//! and apid's, for MCP servers (`crates/apid/src/mcp/egress.rs`, and the HTTP checks in `mcp/http.rs` that call
//! it). Each refuses every address that is not public, and both use the rules here.
//!
//! [`refusal`] classifies one address and names its class. [`in_cidr`] tells whether an address lies in one of
//! the deny blocks apid sends with an egress decision. The blocks that `std` has no predicate for are the
//! [`Cidr`] constants in this file.
//!
//! The rules take an address, never a name. Resolving a host, checking every address it resolves to, and
//! connecting only to the addresses that were checked stay with each proxy.

#![deny(unsafe_code)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A CIDR block: its network address and its prefix length.
pub type Cidr = (IpAddr, u8);

/// The cloud metadata service, `169.254.169.254`.
const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
/// The IPv6 metadata address, `fd00:ec2::254`.
const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

/// "This network", `0.0.0.0/8`.
pub const THIS_NETWORK: Cidr = (IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8);
/// The shared address space, `100.64.0.0/10`: carrier-grade NAT, and the Tailscale tailnet addresses that reach
/// the owner's own machines.
pub const SHARED_ADDRESS_SPACE: Cidr = (IpAddr::V4(Ipv4Addr::new(100, 64, 0, 0)), 10);
/// IETF protocol assignments, `192.0.0.0/24`.
pub const IETF_PROTOCOL_ASSIGNMENTS: Cidr = (IpAddr::V4(Ipv4Addr::new(192, 0, 0, 0)), 24);
/// Benchmarking, `198.18.0.0/15`.
pub const BENCHMARKING: Cidr = (IpAddr::V4(Ipv4Addr::new(198, 18, 0, 0)), 15);
/// Reserved for future use, `240.0.0.0/4`.
pub const RESERVED_V4: Cidr = (IpAddr::V4(Ipv4Addr::new(240, 0, 0, 0)), 4);
/// The IPv4 blocks with no stable `Ipv4Addr` predicate (the IANA special-purpose registry). An address in one
/// of them is refused as "reserved".
pub const SPECIAL_PURPOSE_V4: [Cidr; 5] = [
    THIS_NETWORK,
    SHARED_ADDRESS_SPACE,
    IETF_PROTOCOL_ASSIGNMENTS,
    BENCHMARKING,
    RESERVED_V4,
];

/// Link-local, `fe80::/10`.
pub const LINK_LOCAL_V6: Cidr = (IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)), 10);
/// Site-local (deprecated), `fec0::/10`.
pub const SITE_LOCAL_V6: Cidr = (IpAddr::V6(Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0)), 10);
/// Unique local, `fc00::/7`.
pub const UNIQUE_LOCAL_V6: Cidr = (IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0)), 7);
/// The NAT64 well-known prefix, `64:ff9b::/96` (RFC 6052). The IPv4 address is in its last 32 bits.
pub const NAT64: Cidr = (
    IpAddr::V6(Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0)),
    96,
);
/// The local-use NAT64 prefix, `64:ff9b:1::/48` (RFC 8215).
pub const LOCAL_USE_NAT64: Cidr = (
    IpAddr::V6(Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0)),
    48,
);
/// 6to4, `2002::/16` (RFC 3056). The IPv4 address follows the prefix.
pub const SIX_TO_FOUR: Cidr = (IpAddr::V6(Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0)), 16);
/// Teredo, `2001::/32` (RFC 4380).
pub const TEREDO: Cidr = (IpAddr::V6(Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0)), 32);
/// Documentation, `2001:db8::/32` (RFC 3849).
pub const DOCUMENTATION_V6: Cidr = (
    IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0)),
    32,
);

/// Why `ip` is refused, or `None` when it may be reached. The reason names the class ("loopback", "private",
/// "metadata", ...), and apid's refusal message shows it. An IPv4-mapped IPv6 address is held to the IPv4 rules.
pub fn refusal(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => v4_refusal(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4_refusal(v4),
            None => v6_refusal(v6),
        },
    }
}

/// Whether `ip` lies in `block`. An IPv4-mapped IPv6 address is compared as its IPv4 address, and an IPv4 address
/// never matches an IPv6 block, or the reverse.
pub fn in_cidr(ip: IpAddr, (net, len): Cidr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    match (ip, net) {
        (IpAddr::V4(ip), IpAddr::V4(net)) => {
            let mask = u32::MAX
                .checked_shl(32u32.saturating_sub(u32::from(len)))
                .unwrap_or(0);
            u32::from(ip) & mask == u32::from(net) & mask
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) => {
            let mask = u128::MAX
                .checked_shl(128u32.saturating_sub(u32::from(len)))
                .unwrap_or(0);
            u128::from(ip) & mask == u128::from(net) & mask
        }
        _ => false,
    }
}

/// The class of a refused IPv4 address. The classes are tried in this order, and the first one that holds
/// gives the reason.
fn v4_refusal(ip: Ipv4Addr) -> Option<&'static str> {
    if ip == METADATA_V4 {
        Some("metadata")
    } else if ip.is_unspecified() {
        Some("unspecified")
    } else if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_private() {
        Some("private")
    } else if ip.is_link_local() {
        Some("link-local")
    } else if ip.is_broadcast() {
        Some("broadcast")
    } else if ip.is_multicast() {
        Some("multicast")
    } else if SPECIAL_PURPOSE_V4
        .iter()
        .any(|block| in_cidr(IpAddr::V4(ip), *block))
    {
        Some("reserved")
    } else {
        None
    }
}

/// The class of a refused IPv6 address, in the same order. A NAT64 or 6to4 address that carries an IPv4
/// address is held to the IPv4 rules.
fn v6_refusal(ip: Ipv6Addr) -> Option<&'static str> {
    let addr = IpAddr::V6(ip);
    if ip == METADATA_V6 {
        Some("metadata")
    } else if ip.is_unspecified() {
        Some("unspecified")
    } else if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_multicast() {
        Some("multicast")
    } else if in_cidr(addr, LINK_LOCAL_V6) {
        Some("link-local")
    } else if in_cidr(addr, SITE_LOCAL_V6) {
        Some("site-local")
    } else if in_cidr(addr, UNIQUE_LOCAL_V6) {
        Some("unique-local")
    } else if let Some(v4) = embedded_v4(ip) {
        v4_refusal(v4)
    } else if in_cidr(addr, LOCAL_USE_NAT64) {
        Some("local NAT64")
    } else if in_cidr(addr, TEREDO) || in_cidr(addr, DOCUMENTATION_V6) {
        Some("reserved")
    } else {
        None
    }
}

/// The IPv4 address inside a NAT64 address or a 6to4 one.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let o = ip.octets();
    let addr = IpAddr::V6(ip);
    if in_cidr(addr, NAT64) {
        Some(Ipv4Addr::new(o[12], o[13], o[14], o[15]))
    } else if in_cidr(addr, SIX_TO_FOUR) {
        Some(Ipv4Addr::new(o[2], o[3], o[4], o[5]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn public_addresses_are_allowed() {
        for text in [
            "93.184.216.34",
            "172.32.0.1",
            "8.8.8.8",
            "2606:2800:220:1::1",
            "2001:4860:4860::8888",
            // The neighbours of the special-purpose blocks are public.
            "100.63.255.255",
            "100.128.0.1",
            "192.0.1.1",
            "192.1.0.1",
            "198.17.0.1",
            "198.20.0.1",
            // NAT64 and 6to4 addresses that carry a public IPv4 address are public.
            "64:ff9b::808:808",
            "2002:808:808::1",
        ] {
            assert_eq!(refusal(ip(text)), None, "{text} is public and allowed");
        }
    }

    #[test]
    fn private_loopback_link_local_multicast_and_unspecified_addresses_are_refused() {
        for text in [
            "127.0.0.1",
            "10.0.0.5",
            "10.1.2.3",
            "172.16.0.9",
            "172.31.255.1",
            "192.168.1.1",
            "169.254.10.10",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fc00::5",
            "fd12::5",
            "ff02::1",
            "::ffff:10.0.0.5",
            "::ffff:127.0.0.1",
        ] {
            assert!(refusal(ip(text)).is_some(), "{text} must be refused");
        }
    }

    #[test]
    fn special_purpose_ranges_and_embedded_ipv4_are_refused() {
        for text in [
            "0.1.2.3",
            "100.64.0.1",
            "100.100.100.100",
            "100.127.255.254",
            "192.0.0.8",
            "198.18.0.1",
            "198.19.255.254",
            "240.0.0.1",
            "fec0::1",
            "64:ff9b::7f00:1",
            "64:ff9b::a00:5",
            "64:ff9b:1::5",
            "2002:7f00:1::",
            "2002:c0a8:101::1",
            "2001::1",
            "2001:db8::1",
        ] {
            assert!(refusal(ip(text)).is_some(), "{text} must be refused");
        }
    }

    #[test]
    fn the_metadata_addresses_are_named_as_metadata() {
        assert_eq!(refusal(ip("169.254.169.254")), Some("metadata"));
        assert_eq!(refusal(ip("::ffff:169.254.169.254")), Some("metadata"));
        assert_eq!(refusal(ip("fd00:ec2::254")), Some("metadata"));
        // Their neighbours keep their own classes.
        assert_eq!(refusal(ip("169.254.10.10")), Some("link-local"));
        assert_eq!(refusal(ip("fd00:ec2::255")), Some("unique-local"));
    }

    #[test]
    fn each_refusal_names_its_class() {
        for (text, class) in [
            ("0.0.0.0", "unspecified"),
            ("127.0.0.1", "loopback"),
            ("10.0.0.5", "private"),
            ("169.254.10.10", "link-local"),
            ("255.255.255.255", "broadcast"),
            ("224.0.0.1", "multicast"),
            ("0.1.2.3", "reserved"),
            ("100.64.0.1", "reserved"),
            ("192.0.0.8", "reserved"),
            ("198.18.0.1", "reserved"),
            ("240.0.0.1", "reserved"),
            ("::", "unspecified"),
            ("::1", "loopback"),
            ("ff02::1", "multicast"),
            ("fe80::1", "link-local"),
            ("fec0::1", "site-local"),
            ("fc00::5", "unique-local"),
            ("64:ff9b::a00:5", "private"),
            ("64:ff9b:1::5", "local NAT64"),
            ("2002:7f00:1::", "loopback"),
            ("2001::1", "reserved"),
            ("2001:db8::1", "reserved"),
            ("::ffff:10.0.0.5", "private"),
        ] {
            assert_eq!(refusal(ip(text)), Some(class), "{text}");
        }
    }

    #[test]
    fn the_cidr_constants_hold_their_blocks_and_not_their_neighbours() {
        for (block, inside, outside) in [
            (THIS_NETWORK, "0.255.255.255", "1.0.0.0"),
            (SHARED_ADDRESS_SPACE, "100.127.255.255", "100.128.0.0"),
            (IETF_PROTOCOL_ASSIGNMENTS, "192.0.0.255", "192.0.1.0"),
            (BENCHMARKING, "198.19.255.255", "198.20.0.0"),
            (RESERVED_V4, "255.255.255.254", "239.255.255.255"),
            (LINK_LOCAL_V6, "febf:ffff::1", "fec0::"),
            (SITE_LOCAL_V6, "feff:ffff::1", "ff00::"),
            (UNIQUE_LOCAL_V6, "fdff:ffff::1", "fe00::"),
            (NAT64, "64:ff9b::ffff:ffff", "64:ff9b:0:1::"),
            (LOCAL_USE_NAT64, "64:ff9b:1:ffff::1", "64:ff9b:2::"),
            (SIX_TO_FOUR, "2002:ffff:ffff::1", "2003::"),
            (TEREDO, "2001:0:ffff::1", "2001:1::"),
            (DOCUMENTATION_V6, "2001:db8:ffff::1", "2001:db9::"),
        ] {
            assert!(in_cidr(ip(inside), block), "{inside} is in the block");
            assert!(
                !in_cidr(ip(outside), block),
                "{outside} is outside the block"
            );
        }
    }

    #[test]
    fn a_deny_block_matches_by_prefix_and_never_across_address_families() {
        let block = |text: &str, len: u8| (ip(text), len);
        let v4 = block("8.8.8.0", 24);
        let v6 = block("2001:db8::", 32);
        assert!(in_cidr(ip("8.8.8.8"), v4));
        assert!(
            in_cidr(ip("::ffff:8.8.8.8"), v4),
            "a mapped address is its IPv4 address"
        );
        assert!(!in_cidr(ip("8.8.9.8"), v4));
        assert!(in_cidr(ip("2001:db8::1"), v6));
        assert!(!in_cidr(ip("2001:db9::1"), v6));
        assert!(
            !in_cidr(ip("8.8.8.8"), v6),
            "an IPv4 address never matches an IPv6 block"
        );
        assert!(!in_cidr(ip("2001:db8::1"), v4));
        assert!(
            !in_cidr(ip("::ffff:8.8.8.8"), block("::", 0)),
            "a mapped address is IPv4, so no IPv6 block holds it"
        );
        assert!(in_cidr(ip("1.2.3.4"), block("0.0.0.0", 0)));
    }
}
