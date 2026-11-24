use std::net::IpAddr;

use super::{ip_kind, is_blocked_host_name, IpKind};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Follow,
    Stop,
    TooMany,
}

pub(super) fn decision(
    next: &reqwest::Url,
    pinned_hosts: &[String],
    previous_len: usize,
    allow_private: bool,
) -> Decision {
    if previous_len > 10 {
        return Decision::TooMany;
    }
    if !matches!(next.scheme(), "http" | "https")
        || !next.username().is_empty()
        || next.password().is_some()
    {
        return Decision::Stop;
    }
    let Some(host) = next.host_str() else {
        return Decision::Stop;
    };
    if is_blocked_host_name(host) {
        return Decision::Stop;
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return match ip_kind(ip) {
            IpKind::BlockedMetadata => Decision::Stop,
            IpKind::Private | IpKind::Loopback if !allow_private => Decision::Stop,
            _ => Decision::Follow,
        };
    }
    // Private-network opt-in never authorizes an unclassified DNS lookup.
    if pinned_hosts
        .iter()
        .any(|pinned| pinned.eq_ignore_ascii_case(host))
    {
        Decision::Follow
    } else {
        Decision::Stop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(target: &str, allow_private: bool) -> Decision {
        decision(
            &reqwest::Url::parse(target).unwrap(),
            &["api.example.com".into()],
            1,
            allow_private,
        )
    }

    #[test]
    fn named_redirects_must_have_pinned_addresses_even_with_private_opt_in() {
        for allow_private in [false, true] {
            assert_eq!(
                check("https://API.example.com/v2", allow_private),
                Decision::Follow
            );
            assert_eq!(
                check("https://other.example.com/v2", allow_private),
                Decision::Stop
            );
            assert_eq!(
                check("https://api.example.com./v2", allow_private),
                Decision::Stop
            );
        }
    }

    #[test]
    fn unsafe_urls_and_metadata_never_follow() {
        for target in [
            "http://metadata.google.internal/latest",
            "http://METADATA.google.internal./latest",
            "http://metadata.azure.com./",
            "http://metadata/",
            "http://169.254.169.254/",
            "http://[::ffff:169.254.169.254]/",
            "http://[64:ff9b::a9fe:a9fe]/",
            "http://[2002:a9fe:a9fe::]/",
            "http://[fd00:ec2::254]/",
            "http://[fe80::1]/",
            "ftp://api.example.com/",
            "https://user:password@api.example.com/",
            "file:///etc/passwd",
        ] {
            for allow_private in [false, true] {
                assert_eq!(check(target, allow_private), Decision::Stop, "{target}");
            }
        }
    }

    #[test]
    fn private_literal_redirects_need_explicit_opt_in() {
        for target in [
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::10.0.0.1]/",
            "http://[64:ff9b::7f00:1]/",
            "http://[2002:7f00:1::]/",
            "http://10.0.0.1/",
            "http://[fc00::1]/",
            "http://[fec0::1]/",
        ] {
            assert_eq!(check(target, false), Decision::Stop, "{target}");
            assert_eq!(check(target, true), Decision::Follow, "{target}");
        }
    }

    #[test]
    fn public_literals_do_not_need_dns_and_remain_accessible() {
        for target in [
            "https://8.8.8.8/",
            "https://[2606:4700::1111]/",
            "http://[::ffff:8.8.8.8]/",
        ] {
            assert_eq!(check(target, false), Decision::Follow, "{target}");
        }
    }

    #[test]
    fn redirect_chains_remain_bounded() {
        let url = reqwest::Url::parse("https://api.example.com/").unwrap();
        let pinned = ["api.example.com".into()];
        assert_eq!(decision(&url, &pinned, 10, false), Decision::Follow);
        assert_eq!(decision(&url, &pinned, 11, false), Decision::TooMany);
    }
}
