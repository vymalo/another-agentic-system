use crate::{MAX_URL_BYTES, WindowError};
use orch_ports::is_public_address;
use std::net::IpAddr;
use url::{Host, Url};

/// The URL guard of every adapter (the epic's fourth success metric). Pure: `resolved` is what the
/// adapter resolved the host to (ignored for an IP literal, which is judged as itself).
///
/// Refused as [`WindowError::UrlRefused`]: a URL over [`MAX_URL_BYTES`], one that is not `http` or
/// `https`, one with no host, a name that resolved to nothing, and any address (literal or
/// resolved) that is not public: private, loopback, link-local, metadata, carrier-grade NAT,
/// unique local and the `::ffff:`-mapped and other embedded-IPv4 forms of them.
pub fn check_url(url: &str, resolved: &[IpAddr]) -> Result<(), WindowError> {
    if url.len() > MAX_URL_BYTES {
        return Err(WindowError::UrlRefused);
    }
    let parsed = Url::parse(url).map_err(|_| WindowError::UrlRefused)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(WindowError::UrlRefused);
    }
    match parsed.host() {
        Some(Host::Ipv4(ip)) => judge(&[IpAddr::V4(ip)]),
        Some(Host::Ipv6(ip)) => judge(&[IpAddr::V6(ip)]),
        Some(Host::Domain(_)) => judge(resolved),
        None => Err(WindowError::UrlRefused),
    }
}

fn judge(addrs: &[IpAddr]) -> Result<(), WindowError> {
    if addrs.is_empty() || !addrs.iter().all(|ip| is_public_address(*ip)) {
        return Err(WindowError::UrlRefused);
    }
    Ok(())
}
