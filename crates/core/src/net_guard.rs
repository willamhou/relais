//! Network-egress guard against SSRF (always compiled).
//!
//! Resolves a URL's host and **rejects any resolved IP** in a loopback / private /
//! link-local / ULA / metadata / reserved range, then returns the validated socket
//! addresses so the caller can **pin** them on the HTTP client (resolve-then-
//! connect-to-IP), defeating DNS rebinding. Callers should also disable redirect
//! following (or re-validate each hop) and enforce a host allowlist.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EgressError {
    #[error("invalid url")]
    BadUrl,
    #[error("unsupported url scheme '{0}' (only http/https)")]
    BadScheme(String),
    #[error("url has no host")]
    NoHost,
    #[error("dns resolution failed: {0}")]
    Resolve(String),
    #[error("host did not resolve to any address")]
    NoAddr,
    #[error("blocked egress to non-public address {0}")]
    Blocked(IpAddr),
}

/// A host that passed the egress check, plus the exact IPs to pin.
#[derive(Debug, Clone)]
pub struct GuardedTarget {
    pub host: String,
    pub port: u16,
    pub addrs: Vec<SocketAddr>,
}

/// Parse + validate `url`: http/https only, resolve the host, and reject if **any**
/// resolved address is non-public. On success returns the addresses to pin.
pub fn guard_and_resolve(url: &str) -> Result<GuardedTarget, EgressError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| EgressError::BadUrl)?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(EgressError::BadScheme(other.to_string())),
    }
    let host = parsed.host_str().ok_or(EgressError::NoHost)?.to_string();
    let port = parsed.port_or_known_default().ok_or(EgressError::NoHost)?;

    let addrs: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| EgressError::Resolve(e.to_string()))?
        .collect();
    if addrs.is_empty() {
        return Err(EgressError::NoAddr);
    }
    for a in &addrs {
        if is_blocked_ip(&a.ip()) {
            return Err(EgressError::Blocked(a.ip()));
        }
    }
    Ok(GuardedTarget { host, port, addrs })
}

/// True for any address relais must never connect to for a user-supplied URL:
/// loopback, private (RFC1918), link-local (incl. 169.254.169.254 metadata), CGNAT,
/// benchmarking, reserved/broadcast/unspecified/multicast/documentation; for IPv6:
/// loopback, unspecified, multicast, ULA (fc00::/7), link-local (fe80::/10), and any
/// IPv4-mapped address that is itself blocked.
pub fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            if a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_documentation()
                || a.is_multicast()
            {
                return true;
            }
            let o = a.octets();
            (o[0] == 100 && (o[1] & 0xc0) == 0x40) // 100.64.0.0/10 CGNAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24 IETF
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15 benchmarking
                || o[0] >= 240 // 240.0.0.0/4 reserved
        }
        IpAddr::V6(a) => {
            if a.is_loopback() || a.is_unspecified() || a.is_multicast() {
                return true;
            }
            // Decode embedded IPv4 (IPv4-mapped `::ffff:a.b.c.d` AND IPv4-compatible
            // `::a.b.c.d`) and apply the v4 rules, so e.g. `::ffff:169.254.169.254`
            // can't smuggle a metadata address.
            if let Some(v4) = a.to_ipv4() {
                return is_blocked_ip(&IpAddr::V4(v4));
            }
            let s = a.segments();
            (s[0] & 0xfe00) == 0xfc00              // fc00::/7  unique-local
                || (s[0] & 0xffc0) == 0xfe80       // fe80::/10 link-local
                || s[0] == 0x2002                  // 2002::/16 6to4 (transition; block)
                || (s[0] == 0x2001 && s[1] == 0)   // 2001:0::/32 Teredo
                || (s[0] == 0x0064 && s[1] == 0xff9b) // 64:ff9b::/96 NAT64 well-known
        }
    }
}

/// Does `host` match a cookie `domain` per cookie scoping (exact host, or a
/// subdomain of the domain)? Used to avoid sending stored cookies cross-host.
pub fn host_matches_cookie_domain(host: &str, domain: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let domain = domain
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if domain.is_empty() {
        return false;
    }
    host == domain || host.ends_with(&format!(".{domain}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse::<Ipv4Addr>().unwrap())
    }
    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse::<Ipv6Addr>().unwrap())
    }

    #[test]
    fn blocks_private_and_meta_ranges() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254", // cloud metadata
            "0.0.0.0",
            "100.64.0.1", // CGNAT
            "198.18.0.1", // benchmarking
            "240.0.0.1",  // reserved
            "255.255.255.255",
        ] {
            assert!(is_blocked_ip(&v4(s)), "{s} should be blocked");
        }
        for s in [
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",       // IPv4-mapped loopback
            "::ffff:10.0.0.1",        // IPv4-mapped private
            "::ffff:169.254.169.254", // IPv4-mapped metadata
            "::127.0.0.1",            // IPv4-compatible loopback
            "::a9fe:a9fe",            // IPv4-compatible 169.254.169.254
            "2002:7f00:1::",          // 6to4 encoding 127.0.0.1
            "2002:0a00:0001::",       // 6to4 encoding 10.0.0.1
            "2001::1",                // Teredo
            "64:ff9b::a9fe:a9fe",     // NAT64 well-known → 169.254.169.254
        ] {
            assert!(is_blocked_ip(&v6(s)), "{s} should be blocked");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for s in ["1.1.1.1", "8.8.8.8", "93.184.216.34"] {
            assert!(!is_blocked_ip(&v4(s)), "{s} should be allowed");
        }
        assert!(!is_blocked_ip(&v6("2606:4700:4700::1111")));
    }

    #[test]
    fn guard_blocks_loopback_url_offline() {
        // resolvable without network
        assert!(matches!(
            guard_and_resolve("http://127.0.0.1:8080/x"),
            Err(EgressError::Blocked(_))
        ));
        assert!(matches!(
            guard_and_resolve("http://localhost/x"),
            Err(EgressError::Blocked(_))
        ));
    }

    #[test]
    fn guard_rejects_bad_scheme() {
        assert!(matches!(
            guard_and_resolve("file:///etc/passwd"),
            Err(EgressError::BadScheme(_))
        ));
        assert!(matches!(
            guard_and_resolve("gopher://x"),
            Err(EgressError::BadScheme(_))
        ));
    }

    #[test]
    fn cookie_domain_scoping() {
        assert!(host_matches_cookie_domain("example.com", "example.com"));
        assert!(host_matches_cookie_domain(
            "api.example.com",
            ".example.com"
        ));
        assert!(host_matches_cookie_domain("api.example.com", "example.com"));
        assert!(!host_matches_cookie_domain("evil.com", "example.com"));
        assert!(!host_matches_cookie_domain("notexample.com", "example.com"));
        assert!(!host_matches_cookie_domain(
            "example.com.evil.com",
            "example.com"
        ));
    }
}
