use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
#[cfg(test)]
use serde::Deserialize;
use serde::Serialize;

mod clients;
mod controlled;
pub mod stream;
pub use controlled::{ai_http_stream_controlled, StreamRequest};
mod ip;
mod redirect;
use clients::{client_for, Profile};
use ip::{ip_kind, IpKind};

const HEADER_BLOCKLIST: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "transfer-encoding",
    "upgrade",
    "trailer",
    "expect",
];

fn is_blocked_host_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    matches!(
        host.as_str(),
        "metadata.google.internal" | "metadata" | "metadata.azure.com"
    )
}

/// Resolve `host` once and return both its safety classification and the
/// concrete IPs we resolved. Callers can pin reqwest to these IPs to defeat
/// DNS rebinding (where a second lookup returns a different address).
async fn resolve_and_classify(host: &str) -> Result<(IpKind, Vec<IpAddr>), String> {
    // Direct literal? Skip DNS.
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok((ip_kind(ip), vec![ip]));
    }
    let host_owned = host.to_string();
    let lookup = tokio::task::spawn_blocking(move || {
        (host_owned.as_str(), 0u16)
            .to_socket_addrs()
            .map(|it| it.map(|a| a.ip()).collect::<Vec<_>>())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("dns: {e}"))?;
    if lookup.is_empty() {
        return Err("dns: no addresses".into());
    }
    let mut worst = IpKind::Public;
    for ip in &lookup {
        let k = ip_kind(*ip);
        worst = worst.max(k);
    }
    Ok((worst, lookup))
}

fn validate_url(url: &str, allow_private: bool) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        s => return Err(format!("scheme not allowed: {s}")),
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("userinfo in url is not allowed".into());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "missing host".to_string())?;
    if is_blocked_host_name(host) {
        return Err(format!("host not allowed: {host}"));
    }
    // The actual IP classification has to be async — caller does it.
    let _ = allow_private;
    Ok(parsed)
}

/// Classify the host AND return safe IPs to pin reqwest's resolver to.
/// Defeats DNS rebinding (second-lookup-returns-different-IP) by reusing
/// exactly the addresses that passed `ip_kind`.
async fn classify_and_collect_safe_ips(
    host: &str,
    allow_private: bool,
) -> Result<Vec<IpAddr>, String> {
    let (worst, ips) = resolve_and_classify(host).await?;
    match worst {
        IpKind::BlockedMetadata => return Err(format!("host not allowed: {host}")),
        IpKind::Loopback | IpKind::Private if !allow_private => {
            return Err(format!(
                "host {host} resolves to a private/loopback address; this endpoint requires explicit opt-in",
            ));
        }
        _ => {}
    }
    let safe: Vec<IpAddr> = ips
        .into_iter()
        .filter(|ip| match ip_kind(*ip) {
            IpKind::BlockedMetadata => false,
            IpKind::Loopback | IpKind::Private => allow_private,
            IpKind::Public => true,
        })
        .collect();
    if safe.is_empty() {
        return Err(format!("host {host}: no safe IPs"));
    }
    Ok(safe)
}

fn sanitize_headers(headers: Option<HashMap<String, String>>) -> Result<HeaderMap, String> {
    let mut map = HeaderMap::new();
    let Some(h) = headers else { return Ok(map) };
    for (k, v) in h {
        let lower = k.to_ascii_lowercase();
        if HEADER_BLOCKLIST.contains(&lower.as_str()) {
            return Err(format!("header not allowed: {k}"));
        }
        // CRLF injection: header value must not contain CR / LF / NUL.
        if v.as_bytes().iter().any(|b| matches!(b, 0 | b'\r' | b'\n')) {
            return Err(format!("header value contains control bytes: {k}"));
        }
        let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| e.to_string())?;
        let value = HeaderValue::from_str(&v).map_err(|e| e.to_string())?;
        map.insert(name, value);
    }
    Ok(map)
}

pub async fn lm_ping(base_url: String) -> Result<u16, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("empty base url".into());
    }
    let probe = format!("{trimmed}/models");
    let parsed = validate_url(&probe, true)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .to_string();
    let safe_ips = classify_and_collect_safe_ips(&host, true).await?;

    let client = client_for(Profile::Ping, &host, &safe_ips, true)?;
    client
        .get(parsed)
        .send()
        .await
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string())
}
// AI HTTP proxy — bypasses webview CORS / Mixed-Content / PNA so local-network
// model servers (LM Studio, Ollama, vLLM) work in the production bundle.

#[derive(Debug, Serialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

fn build_request(
    client: &reqwest::Client,
    method: &str,
    url: reqwest::Url,
    headers: Option<HashMap<String, String>>,
    body: Option<Vec<u8>>,
) -> Result<reqwest::RequestBuilder, String> {
    let method = Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut req = client.request(method, url);
    let map = sanitize_headers(headers)?;
    req = req.headers(map);
    if let Some(b) = body {
        req = req.body(b);
    }
    Ok(req)
}

fn header_map_to_strings(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(headers.len());
    for (k, v) in headers {
        if let Ok(s) = v.to_str() {
            out.insert(k.as_str().to_ascii_lowercase(), s.to_string());
        }
    }
    out
}

pub async fn ai_http_request(
    url: String,
    method: String,
    headers: Option<HashMap<String, String>>,
    body: Option<Vec<u8>>,
    allow_private_network: Option<bool>,
) -> Result<HttpResponse, String> {
    let allow_private = allow_private_network.unwrap_or(false);
    let parsed = validate_url(&url, allow_private)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .to_string();
    let safe_ips = classify_and_collect_safe_ips(&host, allow_private).await?;

    let client = client_for(Profile::Stream, &host, &safe_ips, allow_private)?;

    let req = build_request(&client, &method, parsed, headers, body)?;
    let resp = req.send().await.map_err(|e| e.to_string())?;

    let status = resp.status().as_u16();
    let headers = header_map_to_strings(resp.headers());
    let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AiStreamEvent {
    Headers {
        status: u16,
        headers: HashMap<String, String>,
    },
    Chunk {
        bytes: Vec<u8>,
    },
    End,
    Error {
        message: String,
    },
}

#[cfg(test)]
async fn ai_http_stream(
    url: String,
    method: String,
    headers: Option<HashMap<String, String>>,
    body: Option<Vec<u8>>,
    allow_private_network: Option<bool>,
    mut on_event: impl FnMut(AiStreamEvent) -> bool,
) -> Result<(), String> {
    let state = stream::AiStreamState::default();
    let (lease, _) = state.start("test", "request")?;
    let mut received = 0;
    ai_http_stream_controlled(
        StreamRequest {
            url,
            method,
            headers,
            body,
            allow_private_network,
        },
        &lease.control,
        |packet| {
            let event = match packet[0] {
                0 => {
                    #[derive(Deserialize)]
                    struct Headers {
                        status: u16,
                        headers: HashMap<String, String>,
                    }
                    let metadata: Headers = serde_json::from_slice(&packet[1..]).unwrap();
                    AiStreamEvent::Headers {
                        status: metadata.status,
                        headers: metadata.headers,
                    }
                }
                1 => {
                    received += (packet.len() - 1) as u64;
                    state.acknowledge("test", "request", received)?;
                    AiStreamEvent::Chunk {
                        bytes: packet[1..].to_vec(),
                    }
                }
                2 => AiStreamEvent::End,
                _ => AiStreamEvent::Error {
                    message: String::from_utf8_lossy(&packet[1..]).into_owned(),
                },
            };
            if on_event(event) {
                Ok(())
            } else {
                Err("AI consumer closed".into())
            }
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn metadata_ips_classified_as_blocked() {
        // AWS / Google / Azure all share the IPv4 169.254.169.254 link-local.
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))),
            IpKind::BlockedMetadata
        );
        // AWS IPv6 metadata
        assert_eq!(
            ip_kind("fd00:ec2::254".parse().unwrap()),
            IpKind::BlockedMetadata
        );
        // Any link-local IPv4 (169.254/16) — same network range, still blocked.
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))),
            IpKind::BlockedMetadata
        );
        // IPv6 link-local fe80::/10
        assert_eq!(ip_kind("fe80::1".parse().unwrap()), IpKind::BlockedMetadata);
    }

    #[test]
    fn private_ips_classified_correctly() {
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
            IpKind::Private
        );
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))),
            IpKind::Private
        );
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))),
            IpKind::Private
        );
        // CGNAT 100.64/10
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))),
            IpKind::Private
        );
    }

    #[test]
    fn loopback_classified_as_loopback() {
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
            IpKind::Loopback
        );
        assert_eq!(ip_kind("::1".parse().unwrap()), IpKind::Loopback);
    }

    #[test]
    fn public_ips_classified_as_public() {
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))),
            IpKind::Public
        );
        assert_eq!(
            ip_kind(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))),
            IpKind::Public
        );
    }

    #[test]
    fn validate_url_blocks_userinfo_and_metadata_hostnames() {
        // URLs with userinfo can confuse browsers / leak creds in redirects.
        assert!(validate_url("http://user:pass@example.com/", true).is_err());
        // Cloud metadata-by-name.
        assert!(validate_url("http://metadata.google.internal/", true).is_err());
        assert!(validate_url("http://metadata/", true).is_err());
        assert!(validate_url("http://metadata.azure.com/", true).is_err());
        assert!(validate_url("http://METADATA.google.internal./", true).is_err());
        assert!(validate_url("http://metadata.azure.com./", true).is_err());
        assert!(validate_url("http://metadata./", true).is_err());
    }

    #[test]
    fn validate_url_rejects_non_http_schemes() {
        assert!(validate_url("ftp://example.com/", true).is_err());
        assert!(validate_url("file:///etc/passwd", true).is_err());
        assert!(validate_url("javascript:alert(1)", true).is_err());
    }

    #[test]
    fn sanitize_headers_blocks_crlf_injection() {
        let mut h = HashMap::new();
        h.insert("X-Foo".to_string(), "bar\r\nX-Evil: yes".to_string());
        assert!(sanitize_headers(Some(h)).is_err());
    }

    #[test]
    fn sanitize_headers_blocks_hop_by_hop_headers() {
        for hop in [
            "host",
            "content-length",
            "connection",
            "proxy-authorization",
        ] {
            let mut h = HashMap::new();
            h.insert(hop.to_string(), "value".to_string());
            assert!(
                sanitize_headers(Some(h)).is_err(),
                "expected {hop} to be rejected"
            );
        }
    }

    pub(super) fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn ipv6_literal_hosts_are_classified_without_dns() {
        runtime().block_on(async {
            for (host, expected) in [
                ("[::1]", IpKind::Loopback),
                ("[::ffff:127.0.0.1]", IpKind::Loopback),
                ("[::ffff:169.254.169.254]", IpKind::BlockedMetadata),
                ("[64:ff9b::7f00:1]", IpKind::Loopback),
                ("[2002:a00:1::]", IpKind::Private),
                ("[::127.0.0.1]", IpKind::Loopback),
                ("[2606:4700::1111]", IpKind::Public),
                ("[::ffff:8.8.8.8]", IpKind::Public),
            ] {
                let (kind, ips) = resolve_and_classify(host).await.unwrap();
                assert_eq!(kind, expected, "{host}");
                assert_eq!(ips.len(), 1);
                assert_eq!(
                    classify_and_collect_safe_ips(host, false).await.is_err(),
                    expected != IpKind::Public,
                    "{host}"
                );
                assert_eq!(
                    classify_and_collect_safe_ips(host, true).await.is_err(),
                    expected == IpKind::BlockedMetadata,
                    "{host}"
                );
            }
        });
    }

    #[test]
    fn metadata_is_rejected_before_any_request_including_streams_and_pings() {
        runtime().block_on(async {
            for url in [
                "http://[::ffff:169.254.169.254]/",
                "http://[64:ff9b::a9fe:a9fe]/",
                "http://metadata.google.internal./",
            ] {
                assert!(
                    ai_http_request(url.into(), "GET".into(), None, None, Some(true))
                        .await
                        .unwrap_err()
                        .contains("not allowed")
                );
                assert!(lm_ping(url.into())
                    .await
                    .unwrap_err()
                    .contains("not allowed"));
                let mut events = Vec::new();
                let result =
                    ai_http_stream(url.into(), "GET".into(), None, None, Some(true), |event| {
                        events.push(event);
                        true
                    })
                    .await;
                assert!(result.unwrap_err().contains("not allowed"));
                assert!(matches!(&events[..], [AiStreamEvent::Error { .. }]));
            }
        });
    }
}

#[cfg(test)]
mod integration_tests;
