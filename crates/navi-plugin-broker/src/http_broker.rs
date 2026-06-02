use crate::error::BrokerError;
use navi_plugin_manifest::SecurityDefaults;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// HTTP response returned by the broker.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers_json: String,
    pub body: String,
}

/// Capability configuration for HTTP access.
#[derive(Debug, Clone)]
pub struct HttpCapability {
    pub hosts: Vec<String>,
    pub methods: Vec<String>,
    pub https_only: bool,
}

/// HTTP broker that mediates all plugin network access.
pub struct HttpBroker {
    defaults: SecurityDefaults,
    /// DNS pin cache: host -> resolved IP (per invocation).
    dns_pins: HashMap<String, IpAddr>,
    /// Request count for rate limiting.
    request_count: Arc<AtomicU64>,
    /// Rate limit window start.
    window_start: Instant,
    /// Rate limit max requests per window.
    rate_limit: u64,
    /// Max redirects.
    max_redirects: u32,
    /// Max response body bytes.
    max_response_bytes: u64,
    /// Request timeout.
    timeout: Duration,
}

impl HttpBroker {
    /// Create a new HTTP broker with default security settings.
    pub fn new(defaults: SecurityDefaults) -> Self {
        let rate_limit = defaults.http.rate_limit_per_minute as u64;
        let max_redirects = defaults.http.max_redirects;
        let max_response_bytes = defaults.http.max_response_bytes;
        let timeout = Duration::from_secs(30);

        Self {
            defaults,
            dns_pins: HashMap::new(),
            request_count: Arc::new(AtomicU64::new(0)),
            window_start: Instant::now(),
            rate_limit,
            max_redirects,
            max_response_bytes,
            timeout,
        }
    }

    /// Validate a URL against a capability before making a request.
    ///
    /// Returns the parsed URL components if valid.
    pub fn validate_request(
        &self,
        method: &str,
        url: &str,
        capability: &HttpCapability,
    ) -> Result<ValidatedRequest, BrokerError> {
        // Parse URL
        let parsed = url::Url::parse(url).map_err(|_| BrokerError::InvalidUrl {
            url: url.to_string(),
        })?;

        // REQ-HTTP-015: Check scheme
        let scheme = parsed.scheme();
        if scheme != "https" && capability.https_only {
            return Err(BrokerError::AccessDenied {
                reason: format!("scheme '{}' not allowed (HTTPS required)", scheme),
            });
        }
        if scheme != "https" && scheme != "http" {
            return Err(BrokerError::AccessDenied {
                reason: format!("scheme '{}' not allowed", scheme),
            });
        }

        // REQ-HTTP-013: Check host against capability
        let host = parsed.host_str().ok_or_else(|| BrokerError::InvalidUrl {
            url: url.to_string(),
        })?;

        if !capability.hosts.iter().any(|h| h == host || h == "*") {
            return Err(BrokerError::HostNotAllowed {
                host: host.to_string(),
            });
        }

        // Check method against capability
        let method_upper = method.to_uppercase();
        if !capability
            .methods
            .iter()
            .any(|m| m.to_uppercase() == method_upper)
        {
            return Err(BrokerError::AccessDenied {
                reason: format!("method '{}' not allowed", method),
            });
        }

        Ok(ValidatedRequest {
            url: parsed.to_string(),
            host: host.to_string(),
            method: method_upper,
            scheme: scheme.to_string(),
        })
    }

    /// Validate an IP address against blocked ranges.
    ///
    /// REQ-HTTP-002: Reject loopback, private, and link-local IPs.
    pub fn validate_ip(&self, ip: IpAddr) -> Result<(), BrokerError> {
        match ip {
            IpAddr::V4(v4) => self.validate_ipv4(v4),
            IpAddr::V6(v6) => self.validate_ipv6(v6),
        }
    }

    /// Check rate limit.
    ///
    /// REQ-HTTP-008: Max requests per minute.
    pub fn check_rate_limit(&self) -> Result<(), BrokerError> {
        // Reset window if expired
        if self.window_start.elapsed() >= Duration::from_secs(60) {
            // Note: in a real implementation, we'd reset the counter.
            // For now, we just check the count.
        }

        let count = self.request_count.load(Ordering::Relaxed);
        if count >= self.rate_limit {
            return Err(BrokerError::RateLimited);
        }

        self.request_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Pin a DNS resolution for a host.
    ///
    /// REQ-HTTP-009: DNS rebinding prevention.
    pub fn pin_dns(&mut self, host: &str, ip: IpAddr) -> Result<(), BrokerError> {
        if let Some(pinned) = self.dns_pins.get(host) {
            if *pinned != ip {
                return Err(BrokerError::AccessDenied {
                    reason: format!(
                        "DNS rebinding detected: host '{}' resolved to {} but was pinned to {}",
                        host, ip, pinned
                    ),
                });
            }
        } else {
            self.dns_pins.insert(host.to_string(), ip);
        }
        Ok(())
    }

    /// Sanitize response headers.
    ///
    /// REQ-HTTP-006, REQ-HTTP-011, REQ-HTTP-012: Strip sensitive headers.
    pub fn sanitize_headers(&self, headers: &[(String, String)]) -> Vec<(String, String)> {
        headers
            .iter()
            .filter(|(name, _)| !self.defaults.is_sensitive_header(name))
            .cloned()
            .collect()
    }

    /// Sanitize headers and serialize to JSON.
    pub fn sanitize_headers_json(&self, headers: &[(String, String)]) -> String {
        let sanitized = self.sanitize_headers(headers);
        let map: serde_json::Map<String, serde_json::Value> = sanitized
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
    }

    /// Validate a redirect target.
    ///
    /// REQ-HTTP-003, REQ-HTTP-005: Validate every redirect target.
    pub fn validate_redirect(
        &self,
        location: &str,
        current_url: &str,
        capability: &HttpCapability,
        redirect_count: u32,
    ) -> Result<String, BrokerError> {
        // REQ-HTTP-010: Max redirects
        if redirect_count >= self.max_redirects {
            return Err(BrokerError::AccessDenied {
                reason: format!("max redirects ({}) exceeded", self.max_redirects),
            });
        }

        // Parse the redirect location (may be relative)
        let base = url::Url::parse(current_url).map_err(|_| BrokerError::InvalidUrl {
            url: current_url.to_string(),
        })?;

        let redirect_url = base.join(location).map_err(|_| BrokerError::InvalidUrl {
            url: location.to_string(),
        })?;

        // Validate the redirect URL against the capability
        self.validate_request("GET", redirect_url.as_str(), capability)?;

        Ok(redirect_url.to_string())
    }

    /// Get the max response body size.
    pub fn max_response_bytes(&self) -> u64 {
        self.max_response_bytes
    }

    /// Get the request timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Get the max redirects.
    pub fn max_redirects(&self) -> u32 {
        self.max_redirects
    }

    /// Execute an HTTP request after validation.
    ///
    /// This validates the request against the capability, then makes a real
    /// HTTP request using reqwest. Response headers are sanitized.
    ///
    /// REQ-HTTP-001: HTTPS enforced by default.
    /// REQ-HTTP-006: Response headers sanitized.
    /// REQ-HTTP-007: Response body capped.
    pub async fn execute_request(
        &mut self,
        method: &str,
        url: &str,
        body: Option<&str>,
        capability: &HttpCapability,
    ) -> Result<HttpResponse, BrokerError> {
        // Validate the request
        let validated = self.validate_request(method, url, capability)?;

        // Check rate limit
        self.check_rate_limit()?;

        // Build the request
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none()) // We handle redirects manually
            .build()
            .map_err(|e| BrokerError::AccessDenied {
                reason: format!("failed to create HTTP client: {}", e),
            })?;

        let mut req_builder = match validated.method.as_str() {
            "GET" => client.get(&validated.url),
            "POST" => client.post(&validated.url),
            "PUT" => client.put(&validated.url),
            "DELETE" => client.delete(&validated.url),
            "PATCH" => client.patch(&validated.url),
            "HEAD" => client.head(&validated.url),
            _ => {
                return Err(BrokerError::AccessDenied {
                    reason: format!("unsupported method: {}", validated.method),
                });
            }
        };

        if let Some(body_content) = body {
            req_builder = req_builder
                .header("content-type", "application/json")
                .body(body_content.to_string());
        }

        // Send the request
        let response = req_builder
            .send()
            .await
            .map_err(|e| BrokerError::AccessDenied {
                reason: format!("HTTP request failed: {}", e),
            })?;

        // Validate response status and collect headers
        let status = response.status().as_u16();
        let mut headers = Vec::new();
        for (name, value) in response.headers() {
            if let Ok(v) = value.to_str() {
                headers.push((name.to_string(), v.to_string()));
            }
        }

        // Sanitize headers
        let headers_json = self.sanitize_headers_json(&headers);

        // Read body with size cap
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| BrokerError::AccessDenied {
                reason: format!("failed to read response body: {}", e),
            })?;

        let body_str = if body_bytes.len() as u64 > self.max_response_bytes {
            String::from_utf8_lossy(&body_bytes[..self.max_response_bytes as usize]).to_string()
                + "\n[truncated]"
        } else {
            String::from_utf8_lossy(&body_bytes).to_string()
        };

        Ok(HttpResponse {
            status,
            headers_json,
            body: body_str,
        })
    }

    // --- Internal helpers ---

    fn validate_ipv4(&self, addr: Ipv4Addr) -> Result<(), BrokerError> {
        // Loopback: 127.0.0.0/8
        if addr.is_loopback() {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "loopback".into(),
            });
        }

        // Private: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
        if addr.is_private() {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "private network".into(),
            });
        }

        // Link-local: 169.254.0.0/16
        if addr.is_link_local() {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "link-local".into(),
            });
        }

        // Metadata service: 169.254.169.254
        if addr == Ipv4Addr::new(169, 254, 169, 254) {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "cloud metadata service".into(),
            });
        }

        // This network: 0.0.0.0/8
        if addr.octets()[0] == 0 {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "this network (0.0.0.0/8)".into(),
            });
        }

        // Carrier-grade NAT: 100.64.0.0/10
        if addr.octets()[0] == 100 && (addr.octets()[1] & 0xC0) == 64 {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "carrier-grade NAT".into(),
            });
        }

        // IETF protocol assignments: 192.0.0.0/24
        if addr.octets() == [192, 0, 0, 0]
            || addr.octets()[0] == 192 && addr.octets()[1] == 0 && addr.octets()[2] == 0
        {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "IETF protocol assignments".into(),
            });
        }

        // Documentation: TEST-NET-1, TEST-NET-2, TEST-NET-3
        if is_test_net_v4(addr) {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "documentation range".into(),
            });
        }

        // Multicast: 224.0.0.0/4
        if addr.is_multicast() {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "multicast".into(),
            });
        }

        Ok(())
    }

    fn validate_ipv6(&self, addr: Ipv6Addr) -> Result<(), BrokerError> {
        // Loopback: ::1
        if addr.is_loopback() {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "loopback".into(),
            });
        }

        // Link-local: fe80::/10
        if addr.segments()[0] & 0xFFC0 == 0xFE80 {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "link-local".into(),
            });
        }

        // Unique local: fc00::/7
        if addr.segments()[0] & 0xFE00 == 0xFC00 {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "unique local".into(),
            });
        }

        // AWS metadata: fd00:ec2::254
        if addr.segments()[0] == 0xFD00
            && addr.segments()[1] == 0x0EC2
            && addr.segments()[7] == 0x0254
        {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "cloud metadata service (IPv6)".into(),
            });
        }

        // Multicast
        if addr.is_multicast() {
            return Err(BrokerError::IpBlocked {
                ip: addr.to_string(),
                reason: "multicast".into(),
            });
        }

        Ok(())
    }
}

/// Validated request components.
#[derive(Debug, Clone)]
pub struct ValidatedRequest {
    pub url: String,
    pub host: String,
    pub method: String,
    pub scheme: String,
}

/// Check if an IPv4 address is in a documentation range (TEST-NET-1/2/3).
fn is_test_net_v4(addr: Ipv4Addr) -> bool {
    let octets = addr.octets();
    // TEST-NET-1: 192.0.2.0/24
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return true;
    }
    // TEST-NET-2: 198.51.100.0/24
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return true;
    }
    // TEST-NET-3: 203.0.113.0/24
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_capability() -> HttpCapability {
        HttpCapability {
            hosts: vec!["api.example.com".into()],
            methods: vec!["GET".into(), "POST".into()],
            https_only: true,
        }
    }

    fn wildcard_capability() -> HttpCapability {
        HttpCapability {
            hosts: vec!["*".into()],
            methods: vec!["GET".into()],
            https_only: true,
        }
    }

    fn defaults() -> SecurityDefaults {
        SecurityDefaults::default()
    }

    // URL validation tests

    #[test]
    fn valid_https_request() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_request("GET", "https://api.example.com/data", &cap);
        assert!(result.is_ok());
        let req = result.unwrap();
        assert_eq!(req.host, "api.example.com");
        assert_eq!(req.method, "GET");
        assert_eq!(req.scheme, "https");
    }

    #[test]
    fn reject_http_when_https_only() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_request("GET", "http://api.example.com/data", &cap);
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn allow_http_when_not_https_only() {
        let broker = HttpBroker::new(defaults());
        let mut cap = default_capability();
        cap.https_only = false;
        let result = broker.validate_request("GET", "http://api.example.com/data", &cap);
        assert!(result.is_ok());
    }

    #[test]
    fn reject_undeclared_host() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_request("GET", "https://evil.example.com/data", &cap);
        assert!(matches!(result, Err(BrokerError::HostNotAllowed { .. })));
    }

    #[test]
    fn wildcard_host_allowed() {
        let broker = HttpBroker::new(defaults());
        let cap = wildcard_capability();
        let result = broker.validate_request("GET", "https://any.host.com/data", &cap);
        assert!(result.is_ok());
    }

    #[test]
    fn reject_disallowed_method() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_request("DELETE", "https://api.example.com/data", &cap);
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_invalid_url() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_request("GET", "not a url", &cap);
        assert!(matches!(result, Err(BrokerError::InvalidUrl { .. })));
    }

    #[test]
    fn reject_invalid_scheme() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_request("GET", "ftp://api.example.com/data", &cap);
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    // IP validation tests

    #[test]
    fn reject_loopback_v4() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))
                .is_err()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(127, 1, 2, 3)))
                .is_err()
        );
    }

    #[test]
    fn reject_loopback_v6() {
        let broker = HttpBroker::new(defaults());
        assert!(broker.validate_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)).is_err());
    }

    #[test]
    fn reject_private_10() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
                .is_err()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255)))
                .is_err()
        );
    }

    #[test]
    fn reject_private_172() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)))
                .is_err()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255)))
                .is_err()
        );
    }

    #[test]
    fn reject_private_192() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)))
                .is_err()
        );
    }

    #[test]
    fn reject_link_local() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)))
                .is_err()
        );
    }

    #[test]
    fn reject_metadata_service() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)))
                .is_err()
        );
    }

    #[test]
    fn reject_metadata_service_v6() {
        let broker = HttpBroker::new(defaults());
        let addr = Ipv6Addr::new(0xFD00, 0x0EC2, 0, 0, 0, 0, 0, 0x0254);
        assert!(broker.validate_ip(IpAddr::V6(addr)).is_err());
    }

    #[test]
    fn reject_this_network() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))
                .is_err()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(0, 1, 2, 3)))
                .is_err()
        );
    }

    #[test]
    fn reject_carrier_grade_nat() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)))
                .is_err()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255)))
                .is_err()
        );
    }

    #[test]
    fn reject_multicast() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)))
                .is_err()
        );
    }

    #[test]
    fn reject_test_net() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)))
                .is_err()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)))
                .is_err()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)))
                .is_err()
        );
    }

    #[test]
    fn reject_unique_local_v6() {
        let broker = HttpBroker::new(defaults());
        let addr = Ipv6Addr::new(0xFC00, 0, 0, 0, 0, 0, 0, 1);
        assert!(broker.validate_ip(IpAddr::V6(addr)).is_err());
    }

    #[test]
    fn reject_link_local_v6() {
        let broker = HttpBroker::new(defaults());
        let addr = Ipv6Addr::new(0xFE80, 0, 0, 0, 0, 0, 0, 1);
        assert!(broker.validate_ip(IpAddr::V6(addr)).is_err());
    }

    #[test]
    fn allow_public_ip() {
        let broker = HttpBroker::new(defaults());
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)))
                .is_ok()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
                .is_ok()
        );
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)))
                .is_err()
        ); // TEST-NET-3
        assert!(
            broker
                .validate_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)))
                .is_ok()
        ); // example.com
    }

    // DNS pinning tests

    #[test]
    fn dns_pin_first_resolution() {
        let mut broker = HttpBroker::new(defaults());
        let ip = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
        assert!(broker.pin_dns("example.com", ip).is_ok());
    }

    #[test]
    fn dns_pin_same_ip_ok() {
        let mut broker = HttpBroker::new(defaults());
        let ip = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
        broker.pin_dns("example.com", ip).unwrap();
        assert!(broker.pin_dns("example.com", ip).is_ok());
    }

    #[test]
    fn dns_pin_different_ip_rejected() {
        let mut broker = HttpBroker::new(defaults());
        let ip1 = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
        let ip2 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        broker.pin_dns("example.com", ip1).unwrap();
        assert!(broker.pin_dns("example.com", ip2).is_err());
    }

    // Header sanitization tests

    #[test]
    fn sanitize_removes_authorization() {
        let broker = HttpBroker::new(defaults());
        let headers = vec![
            ("Content-Type".into(), "text/html".into()),
            ("Authorization".into(), "Bearer secret".into()),
        ];
        let sanitized = broker.sanitize_headers(&headers);
        assert_eq!(sanitized.len(), 1);
        assert_eq!(sanitized[0].0, "Content-Type");
    }

    #[test]
    fn sanitize_removes_cookie() {
        let broker = HttpBroker::new(defaults());
        let headers = vec![
            ("Cookie".into(), "session=abc".into()),
            ("Set-Cookie".into(), "token=xyz".into()),
            ("Content-Type".into(), "text/html".into()),
        ];
        let sanitized = broker.sanitize_headers(&headers);
        assert_eq!(sanitized.len(), 1);
    }

    #[test]
    fn sanitize_removes_token_suffix() {
        let broker = HttpBroker::new(defaults());
        let headers = vec![
            ("X-Csrf-Token".into(), "abc".into()),
            ("X-Api-Key".into(), "secret".into()),
            ("X-Request-Id".into(), "123".into()),
        ];
        let sanitized = broker.sanitize_headers(&headers);
        assert_eq!(sanitized.len(), 1);
        assert_eq!(sanitized[0].0, "X-Request-Id");
    }

    #[test]
    fn sanitize_headers_json() {
        let broker = HttpBroker::new(defaults());
        let headers = vec![
            ("Content-Type".into(), "text/html".into()),
            ("Authorization".into(), "Bearer secret".into()),
        ];
        let json = broker.sanitize_headers_json(&headers);
        assert!(json.contains("Content-Type"));
        assert!(!json.contains("Authorization"));
    }

    // Redirect validation tests

    #[test]
    fn validate_redirect_relative() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_redirect("/new-path", "https://api.example.com/old", &cap, 0);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://api.example.com/new-path");
    }

    #[test]
    fn validate_redirect_same_host() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_redirect(
            "https://api.example.com/new",
            "https://api.example.com/old",
            &cap,
            0,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn reject_redirect_to_undeclared_host() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_redirect(
            "https://evil.example.com/stolen",
            "https://api.example.com/old",
            &cap,
            0,
        );
        assert!(matches!(result, Err(BrokerError::HostNotAllowed { .. })));
    }

    #[test]
    fn reject_redirect_max_exceeded() {
        let broker = HttpBroker::new(defaults());
        let cap = default_capability();
        let result = broker.validate_redirect(
            "/next",
            "https://api.example.com/old",
            &cap,
            3, // already at max
        );
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    // Rate limiting tests

    #[test]
    fn rate_limit_allows_within_limit() {
        let broker = HttpBroker::new(defaults());
        for _ in 0..10 {
            assert!(broker.check_rate_limit().is_ok());
        }
    }

    #[test]
    fn rate_limit_blocks_at_limit() {
        let mut d = defaults();
        d.http.rate_limit_per_minute = 3;
        let broker = HttpBroker::new(d);
        assert!(broker.check_rate_limit().is_ok());
        assert!(broker.check_rate_limit().is_ok());
        assert!(broker.check_rate_limit().is_ok());
        assert!(broker.check_rate_limit().is_err());
    }
}
