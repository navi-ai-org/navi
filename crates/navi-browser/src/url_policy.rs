//! URL safety checks for browser navigation (SSRF mitigation).

use std::net::IpAddr;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UrlPolicyError {
    #[error("invalid URL: {0}")]
    Invalid(String),
    #[error("URL scheme `{0}` is not allowed (use http or https)")]
    Scheme(String),
    #[error("URL host is missing")]
    MissingHost,
    #[error("navigation to private/local network is blocked: {0}")]
    PrivateNetwork(String),
}

/// Validate a navigation URL.
///
/// - Only `http` / `https`
/// - Blocks `file:`, `javascript:`, etc.
/// - When `allow_private_network` is false, blocks localhost, link-local, and RFC1918
pub fn validate_navigation_url(raw: &str, allow_private_network: bool) -> Result<Url, UrlPolicyError> {
    let url = Url::parse(raw.trim()).map_err(|e| UrlPolicyError::Invalid(e.to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(UrlPolicyError::Scheme(other.to_string())),
    }
    let host = url
        .host_str()
        .ok_or(UrlPolicyError::MissingHost)?
        .to_ascii_lowercase();

    if !allow_private_network && is_private_or_local_host(&host) {
        return Err(UrlPolicyError::PrivateNetwork(host));
    }
    Ok(url)
}

fn is_private_or_local_host(host: &str) -> bool {
    if host == "localhost"
        || host.ends_with(".localhost")
        || host == "0.0.0.0"
        || host == "::1"
        || host == "[::1]"
    {
        return true;
    }
    // Strip IPv6 brackets if present.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.octets()[0] == 100 && (v4.octets()[1] & 0b1100_0000) == 0b0100_0000 // CGNAT 100.64/10
            }
            IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || is_unique_local_v6(v6),
        };
    }
    false
}

fn is_unique_local_v6(ip: std::net::Ipv6Addr) -> bool {
    // fc00::/7
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_public_https() {
        assert!(validate_navigation_url("https://example.com/path", false).is_ok());
    }

    #[test]
    fn rejects_file_scheme() {
        assert!(matches!(
            validate_navigation_url("file:///etc/passwd", false),
            Err(UrlPolicyError::Scheme(_))
        ));
    }

    #[test]
    fn rejects_localhost_by_default() {
        assert!(matches!(
            validate_navigation_url("http://127.0.0.1:3000", false),
            Err(UrlPolicyError::PrivateNetwork(_))
        ));
        assert!(matches!(
            validate_navigation_url("http://localhost:8080", false),
            Err(UrlPolicyError::PrivateNetwork(_))
        ));
    }

    #[test]
    fn allows_localhost_when_configured() {
        assert!(validate_navigation_url("http://127.0.0.1:3000", true).is_ok());
        assert!(validate_navigation_url("http://localhost:5173", true).is_ok());
    }

    #[test]
    fn rejects_rfc1918() {
        assert!(validate_navigation_url("http://192.168.1.1/", false).is_err());
        assert!(validate_navigation_url("http://10.0.0.5/", false).is_err());
    }
}
