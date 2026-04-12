//! DNS-aware SSRF protection for webhook URLs.
//!
//! Provides a custom [`reqwest::dns::Resolve`] implementation that resolves
//! hostnames and rejects any addresses that point to private, loopback,
//! link-local, or otherwise internal IP ranges.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Returns `true` if the given IP address is considered internal/blocked for
/// outbound webhook requests.
pub fn is_ip_blocked(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || is_v4_shared(v4)
                || is_v4_benchmarking(v4)
                || is_v4_reserved(v4)
                // Cloud metadata endpoints (169.254.169.254)
                || *v4 == Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped (::ffff:x.x.x.x) — check embedded IPv4
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_ip_blocked(&IpAddr::V4(mapped));
            }
            // 6to4 (2002::/16) — embeds external IPv4 in bits 16-48
            if v6.segments()[0] == 0x2002 {
                let ipv4 = Ipv4Addr::new(
                    (v6.segments()[1] >> 8) as u8,
                    v6.segments()[1] as u8,
                    (v6.segments()[2] >> 8) as u8,
                    v6.segments()[2] as u8,
                );
                return is_ip_blocked(&IpAddr::V4(ipv4));
            }
            // Teredo (2001:0000::/32) — can tunnel to arbitrary hosts
            if v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0000 {
                return true;
            }
            v6.is_loopback() || v6.is_unspecified()
            // Note: v6.is_unicast_link_local() is unstable; check manually
                || (v6.segments()[0] & 0xffc0) == 0xfe80
            // ULA (fc00::/7)
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

/// 100.64.0.0/10 — shared address space (RFC 6598)
fn is_v4_shared(ip: &Ipv4Addr) -> bool {
    ip.octets()[0] == 100 && (ip.octets()[1] & 0xC0) == 64
}

/// 198.18.0.0/15 — benchmarking (RFC 2544)
fn is_v4_benchmarking(ip: &Ipv4Addr) -> bool {
    ip.octets()[0] == 198 && (ip.octets()[1] == 18 || ip.octets()[1] == 19)
}

/// 240.0.0.0/4 — reserved for future use
fn is_v4_reserved(ip: &Ipv4Addr) -> bool {
    ip.octets()[0] >= 240
}

/// A [`Resolve`] implementation that performs standard DNS resolution and then
/// filters out any addresses pointing to blocked (internal) IP ranges.
///
/// If *all* resolved addresses are blocked, resolution fails with an SSRF error.
#[derive(Clone)]
pub struct SsrfSafeResolver;

impl Resolve for SsrfSafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            // Use tokio's built-in DNS resolution (delegates to getaddrinfo)
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host(format!("{host}:0"))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("DNS resolution failed for {host}: {e}").into()
                })?
                .collect();

            if addrs.is_empty() {
                return Err(format!("DNS resolution returned no addresses for {host}").into());
            }

            let safe: Vec<SocketAddr> =
                addrs.into_iter().filter(|a| !is_ip_blocked(&a.ip())).collect();

            if safe.is_empty() {
                return Err(format!(
                    "webhook URL blocked: all resolved addresses for '{host}' are internal/private (SSRF protection)"
                ).into());
            }

            let addrs: Addrs = Box::new(safe.into_iter());
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_blocked_ips() {
        assert!(is_ip_blocked(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_ip_blocked(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_ip_blocked(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_ip_blocked(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(is_ip_blocked(&IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(is_ip_blocked(&IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert!(is_ip_blocked(&IpAddr::V4(Ipv4Addr::BROADCAST)));
        assert!(is_ip_blocked(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_ip_blocked(&IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn test_allowed_ips() {
        assert!(!is_ip_blocked(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_ip_blocked(&IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!is_ip_blocked(&IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
    }

    #[test]
    fn test_ipv4_mapped_ipv6_blocked() {
        // ::ffff:127.0.0.1
        assert!(is_ip_blocked(&IpAddr::V6("::ffff:127.0.0.1".parse().unwrap())));
        // ::ffff:10.0.0.1
        assert!(is_ip_blocked(&IpAddr::V6("::ffff:10.0.0.1".parse().unwrap())));
        // ::ffff:192.168.1.1
        assert!(is_ip_blocked(&IpAddr::V6("::ffff:192.168.1.1".parse().unwrap())));
        // ::ffff:169.254.169.254 (cloud metadata)
        assert!(is_ip_blocked(&IpAddr::V6("::ffff:169.254.169.254".parse().unwrap())));
    }

    #[test]
    fn test_ipv4_mapped_ipv6_allowed() {
        // ::ffff:8.8.8.8 should be allowed
        assert!(!is_ip_blocked(&IpAddr::V6("::ffff:8.8.8.8".parse().unwrap())));
        // ::ffff:1.1.1.1 should be allowed
        assert!(!is_ip_blocked(&IpAddr::V6("::ffff:1.1.1.1".parse().unwrap())));
    }

    #[test]
    fn test_6to4_blocked() {
        // 2002:7f00:0001:: embeds 127.0.0.1
        assert!(is_ip_blocked(&IpAddr::V6("2002:7f00:0001::".parse().unwrap())));
        // 2002:0a00:0001:: embeds 10.0.0.1
        assert!(is_ip_blocked(&IpAddr::V6("2002:0a00:0001::".parse().unwrap())));
    }

    #[test]
    fn test_6to4_allowed() {
        // 2002:0808:0808:: embeds 8.8.8.8 — should be allowed
        assert!(!is_ip_blocked(&IpAddr::V6("2002:0808:0808::".parse().unwrap())));
    }

    #[test]
    fn test_teredo_blocked() {
        assert!(is_ip_blocked(&IpAddr::V6("2001:0000::1".parse().unwrap())));
        assert!(is_ip_blocked(&IpAddr::V6(
            "2001:0000:4136:e378:8000:63bf:3fff:fdd2".parse().unwrap()
        )));
    }
}
