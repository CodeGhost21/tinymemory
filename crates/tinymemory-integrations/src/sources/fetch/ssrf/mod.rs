//! Shared SSRF guard and fetch hygiene for the network source readers.
//!
//! [`super::fetch_url`] and the web-page and RSS readers built on it all fetch
//! user-configured URLs, so they share the policy in this module. It is public
//! so a host fetching a user-supplied URL by other means applies the same
//! policy rather than a second, weaker one.
//!
//! The hostname *text* check (`is_blocked_host`) rejects private IP literals
//! (including their IPv4-mapped IPv6 forms, e.g. `::ffff:127.0.0.1`),
//! `localhost`, `.local` / `.internal` names, and single-label hostnames, but
//! a public-looking name can resolve to a loopback / private / link-local
//! address (including the cloud-metadata `169.254.169.254`) at lookup time.
//! `PublicOnlyResolver` therefore vets the resolved addresses and only lets the
//! connection proceed to a globally routable IP, so the request is pinned to an
//! address we have already allowed (no re-resolution between the check and the
//! connect). Redirects are re-checked through `is_url_allowed` so a public URL
//! cannot bounce the fetch onto an internal host.
//!
//! `read_body_capped` streams a response body and stops at a byte cap, so a
//! hostile or gigantic page/feed cannot OOM the process before the size check
//! runs.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use futures::stream::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Build an HTTP client with a redirect policy that re-applies the SSRF
/// host/scheme check to every redirect hop, a DNS resolver that only yields
/// globally routable addresses, a 20-second timeout and the `openhuman`
/// user agent the network readers have always sent.
///
/// # Errors
///
/// A message naming the failure when the TLS backend cannot be initialised.
pub fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("openhuman")
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if is_url_allowed(attempt.url()) {
                attempt.follow()
            } else {
                // `stop` returns the redirect response to the caller instead
                // of following it; the read then fails on the non-2xx status.
                attempt.stop()
            }
        }))
        .dns_resolver(Arc::new(PublicOnlyResolver))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))
}

/// Stream a response body, failing once it exceeds `max` bytes.
///
/// `Response::bytes()` buffers the entire body before any size check, so a
/// server that omits or understates `Content-Length` (for example a chunked
/// response) could OOM the process despite the cap. Reading incrementally
/// enforces the limit while the bytes arrive.
///
/// # Errors
///
/// A message containing `exceeds {max}-byte limit` when the body is over the
/// cap, or `failed to read response body` when the stream fails mid-read.
pub async fn read_body_capped(resp: reqwest::Response, max: u64) -> Result<Vec<u8>, String> {
    // Trust a truthful Content-Length up front so a known-huge body is
    // rejected before the first byte is read.
    if let Some(len) = resp.content_length()
        && len > max
    {
        return Err(format!(
            "response body exceeds {max}-byte limit (Content-Length={len})"
        ));
    }

    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("failed to read response body: {e}"))?;
        body.extend_from_slice(&chunk);
        if body.len() as u64 > max {
            return Err(format!(
                "response body exceeds {max}-byte limit (read {} bytes)",
                body.len()
            ));
        }
    }
    Ok(body)
}

/// A DNS resolver that only yields globally routable addresses.
///
/// The text-based `is_blocked_host` check rejects private IP *literals* and
/// local hostnames, but a public-looking hostname can resolve to a loopback,
/// private, link-local, or cloud-metadata address (`169.254.169.254`) at
/// lookup time. Installing this resolver means reqwest connects to addresses
/// we have already vetted: a hostname whose current resolution is non-public
/// fails the request instead of silently reaching an internal service, and the
/// validated address is the one the connection is pinned to (no re-resolution
/// between the check and the connect).
#[derive(Debug, Default)]
struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(box_err)?
                .filter(|addr| is_public_ip(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(box_err(std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    format!("host {host} resolved to no public addresses"),
                )));
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn box_err(
    e: impl std::error::Error + Send + Sync + 'static,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(e)
}

/// Whether `ip` is a globally routable address — the one address classifier
/// behind both halves of the SSRF guard (literal hosts in `is_blocked_host`
/// and resolved addresses in `PublicOnlyResolver`).
///
/// Not fetchable: loopback, private, link-local, unspecified, CGNAT
/// (`100.64.0.0/10`), `192.0.0.0/16` (IETF protocol assignments and the
/// `192.0.2.0/24` documentation range), multicast, broadcast, documentation
/// (`198.51.100.0/24`, `203.0.113.0/24`, `2001:db8::/32`), benchmarking
/// (`198.18.0.0/15`),
/// reserved (`240.0.0.0/4`), and IPv6 unique-local (`fc00::/7`) and
/// link-local (`fe80::/10`). An IPv6 address carrying an IPv4 one — mapped
/// (`::ffff:a.b.c.d`) or the deprecated compatible form (`::a.b.c.d`) — is
/// judged by its IPv4 part, so a mapped loopback stays blocked.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || (o[0] == 100 && o[1] & 0xc0 == 0x40)
                || (o[0] == 192 && o[1] == 0)
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
                || (o[0] == 198 && o[1] & 0xfe == 18)
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            let o = v6.octets();
            if (o[0] & 0xfe == 0xfc)
                || (o[0] == 0xfe && o[1] & 0xc0 == 0x80)
                || (o[0] == 0x20 && o[1] == 0x01 && o[2] == 0x0d && o[3] == 0xb8)
            {
                return false;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            // `to_ipv4_mapped` answers `None` for the compatible form, so
            // without this `::127.0.0.1` would read as public. `::` and `::1`
            // were judged as themselves above.
            if o[..12].iter().all(|byte| *byte == 0) {
                return is_public_ip(IpAddr::V4(Ipv4Addr::new(o[12], o[13], o[14], o[15])));
            }
            true
        }
    }
}

/// Whether a URL may be fetched: `http(s)` scheme against a public host.
pub fn is_url_allowed(url: &reqwest::Url) -> bool {
    match url.scheme() {
        "http" | "https" => {}
        _ => return false,
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    !is_blocked_host(host)
}

/// Reject hosts that could target non-public resources: IP literals in
/// loopback / private / link-local / unique-local / unspecified ranges (and
/// their IPv4-mapped IPv6 forms), plus `localhost`, `.local` / `.internal`
/// names, and single-label hostnames (internal service names such as `mongo`
/// or `redis`).
fn is_blocked_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        // A literal never goes through DNS resolution, so `PublicOnlyResolver`
        // never sees it — this text check is its only line of defense, and it
        // uses the same classification as the resolver.
        return !is_public_ip(ip);
    }
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".internal") {
        return true;
    }
    // A single-label name is an internal-service name, not a public domain.
    !host.contains('.')
}

#[cfg(test)]
#[path = "ssrf_tests.rs"]
mod tests;
