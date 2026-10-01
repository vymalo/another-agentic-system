//! What a `Host` header value is, shared by the surfaces that check it (MCP, thread tools).

/// Whether `host` is a `Host` value: a name or an address, with or without a port, and nothing
/// else (no scheme, credentials, path or wildcard). A value that is not one matches no request,
/// so an allow-list holding it would only look like a rule.
pub fn is_host_authority(host: &str) -> bool {
    if host.contains(['@', '*', '/', '?', '#', ' ']) {
        return false;
    }
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    if authority.as_str() != host {
        return false;
    }
    // A port, if there is one, is a number: anything else after a colon parses as a strange host
    // name, and matches nothing.
    let after_host = &host[authority.host().len()..];
    match after_host.strip_prefix(':') {
        None => after_host.is_empty() && !authority.host().is_empty(),
        Some(port) => {
            !authority.host().is_empty()
                && !port.is_empty()
                && port.bytes().all(|b| b.is_ascii_digit())
                && authority.port_u16().is_some()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_authorities_and_nothing_else() {
        for ok in [
            "localhost",
            "localhost:8080",
            "orch.example.com",
            "orch.example.com:443",
            "127.0.0.1:8080",
            "[::1]:8080",
        ] {
            assert!(is_host_authority(ok), "{ok}");
        }
        for bad in [
            "",
            "*",
            "*.example.com",
            "https://orch.example.com",
            "orch.example.com/",
            "orch.example.com/mcp",
            "user@orch.example.com",
            "orch.example.com:",
            "orch.example.com:notaport",
            "orch.example.com:99999",
            ":8080",
            "orch example.com",
        ] {
            assert!(!is_host_authority(bad), "{bad:?}");
        }
    }
}
