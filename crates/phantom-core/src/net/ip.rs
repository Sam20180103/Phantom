use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
pub(super) enum IpKind {
    Public,
    Private,
    Loopback,
    BlockedMetadata,
}

pub(super) fn ip_kind(ip: IpAddr) -> IpKind {
    match ip {
        IpAddr::V4(ip) => ipv4_kind(ip),
        IpAddr::V6(ip) => ipv6_kind(ip),
    }
}

fn ipv4_kind(ip: Ipv4Addr) -> IpKind {
    let o = ip.octets();
    if ip.is_link_local() {
        return IpKind::BlockedMetadata;
    }
    if ip.is_loopback() || o[0] == 0 || ip.is_broadcast() || ip.is_multicast() {
        return IpKind::Loopback;
    }
    if o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 100 && (64..=127).contains(&o[1]))
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || (o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2))
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || o[0] >= 240
    {
        return IpKind::Private;
    }
    IpKind::Public
}

fn ipv6_kind(ip: Ipv6Addr) -> IpKind {
    let s = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return IpKind::Loopback;
    }
    if (s[0] == 0xfd00 && s[1] == 0xec2) || s[0] & 0xffc0 == 0xfe80 {
        return IpKind::BlockedMetadata;
    }
    if let Some(embedded) = embedded_ipv4(&s) {
        return embedded
            .into_iter()
            .flatten()
            .map(ipv4_kind)
            .max()
            .unwrap_or(IpKind::Public);
    }
    if s[0] & 0xe000 != 0x2000
        || (s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
        || (s[0] == 0x3fff && s[1] < 0x1000)
    {
        return IpKind::Private;
    }
    IpKind::Public
}

fn embedded_ipv4(s: &[u16; 8]) -> Option<[Option<Ipv4Addr>; 2]> {
    let v4 = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Some(Ipv4Addr::new(a, b, c, d))
    };
    match s {
        [0, 0, 0, 0, 0, 0xffff, hi, lo]
        | [0, 0, 0, 0, 0, 0, hi, lo]
        | [0, 0, 0, 0, 0xffff, 0, hi, lo]
        | [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] => Some([v4(*hi, *lo), None]),
        [0x2002, hi, lo, ..] => Some([v4(*hi, *lo), None]),
        [0x2001, 0, server_hi, server_lo, _, _, client_hi, client_lo] => {
            Some([v4(*server_hi, *server_lo), v4(!*client_hi, !*client_lo)])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(address: &str) -> IpKind {
        ip_kind(address.parse().unwrap())
    }

    #[test]
    fn mapped_compatible_and_translated_addresses_use_the_embedded_ipv4() {
        for prefix in ["::ffff:", "::", "::ffff:0:"] {
            for (address, expected) in [
                ("127.0.0.1", IpKind::Loopback),
                ("169.254.169.254", IpKind::BlockedMetadata),
                ("10.0.0.1", IpKind::Private),
                ("192.168.1.1", IpKind::Private),
                ("172.16.0.1", IpKind::Private),
                ("100.64.0.1", IpKind::Private),
                ("0.0.0.0", IpKind::Loopback),
                ("8.8.8.8", IpKind::Public),
            ] {
                assert_eq!(
                    kind(&format!("{prefix}{address}")),
                    expected,
                    "{prefix}{address}"
                );
            }
        }
        assert_eq!(kind("::ffff:7f00:1"), IpKind::Loopback);
        assert_eq!(kind("::ffff:a9fe:a9fe"), IpKind::BlockedMetadata);
    }

    #[test]
    fn nat64_and_six_to_four_use_the_embedded_ipv4() {
        for (suffix, expected) in [
            ("7f00:1", IpKind::Loopback),
            ("a9fe:a9fe", IpKind::BlockedMetadata),
            ("a00:1", IpKind::Private),
            ("c0a8:101", IpKind::Private),
            ("808:808", IpKind::Public),
        ] {
            assert_eq!(kind(&format!("64:ff9b::{suffix}")), expected);
            assert_eq!(kind(&format!("2002:{suffix}::1")), expected);
        }
        assert_eq!(kind("64:ff9b:1::808:808"), IpKind::Private);
    }

    #[test]
    fn teredo_uses_the_most_restricted_server_or_client() {
        for (address, expected) in [
            ("2001:0:808:808:0:0:fefd:fcfb", IpKind::Public),
            ("2001:0:808:808:0:0:80ff:fffe", IpKind::Loopback),
            ("2001:0:a00:1:0:0:fefd:fcfb", IpKind::Private),
            ("2001:0:808:808:0:0:5601:5601", IpKind::BlockedMetadata),
            ("2001:0:a9fe:a9fe:0:0:80ff:fffe", IpKind::BlockedMetadata),
        ] {
            assert_eq!(kind(address), expected, "{address}");
        }
    }

    #[test]
    fn non_global_ipv6_ranges_are_not_public() {
        for address in [
            "fc00::1",
            "fd12:3456::1",
            "fdff:ffff::1",
            "fec0::1",
            "feff::1",
            "100::1",
            "2001:db8::1",
            "2001:db8:ffff::1",
            "2001:2::1",
            "3fff::1",
            "3fff:fff:ffff::1",
            "4000::1",
            "8000::1",
        ] {
            assert_eq!(kind(address), IpKind::Private, "{address}");
        }
        for address in ["fe80::1", "febf::1", "fd00:ec2::254"] {
            assert_eq!(kind(address), IpKind::BlockedMetadata, "{address}");
        }
        for address in ["::", "::1", "ff02::1", "ff00::"] {
            assert_eq!(kind(address), IpKind::Loopback, "{address}");
        }
    }

    #[test]
    fn global_unicast_ipv6_remains_public() {
        for address in [
            "2606:4700::1111",
            "2607:f8b0:4005:80a::200e",
            "2a00:1450:4001:81b::200e",
            "2001:4860:4860::8888",
            "2001:200::1",
            "2001:1200::1",
            "2620:fe::fe",
            "3fff:1000::1",
            "3ffe::1",
        ] {
            assert_eq!(kind(address), IpKind::Public, "{address}");
        }
    }

    #[test]
    fn reserved_and_documentation_ipv4_are_not_public() {
        for address in [
            "0.1.2.3",
            "192.0.0.8",
            "192.0.2.1",
            "198.51.100.7",
            "203.0.113.9",
            "240.0.0.1",
            "255.255.255.254",
        ] {
            assert_ne!(kind(address), IpKind::Public, "{address}");
        }
        for address in ["192.0.1.1", "198.51.101.1", "203.0.114.1"] {
            assert_eq!(kind(address), IpKind::Public, "{address}");
        }
    }
}
