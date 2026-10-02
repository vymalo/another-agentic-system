//! Files an agent points at instead of sending (ADR 0032): a `url` part of an artifact.
//!
//! The mapper keeps such a part as the artifact's `uri`: it does no I/O. This adapter reads it
//! from the host's own side of the wire when, and only when, the URL's host is on the operator's
//! allow-list (`artifacts.fetchHosts`, empty by default), and hands the bytes on as
//! [`AgentUpdate::File`] exactly as if the agent had sent a `raw` part. Anything else stays a link.
//!
//! The allow-list is the SSRF control: an agent chooses the URL, the operator chooses the hosts, and
//! a host on the list is trusted. Besides it the fetch is `http` or `https` only, the URL has no
//! credentials, a redirect is **never followed** (it would leave the list), no credential of
//! the agent or the thread is sent, a response is read piece by piece and stopped at the cap (never
//! held whole before it is checked), and the whole request has a time limit. (The agent cards the
//! orchestrator reads come from the operator's own configuration and have no such check: there is
//! no helper to reuse, and this one is not for them.)

use std::time::Duration;

use orch_core::{AgentUpdate, FileRefusal};
use orch_ports::{AgentEnvelope, TaskSnapshot};
use url::Url;

/// What may be fetched, from where.
#[derive(Debug, Clone)]
pub struct FileFetch {
    /// `artifacts.fetchHosts`: `host` (the scheme's default port) or `host:port`, compared in
    /// lower case.
    pub hosts: Vec<String>,
    /// `artifacts.maxFileBytes`: a longer response is refused.
    pub max_bytes: u64,
    /// The whole request, connect to last byte.
    pub timeout: Duration,
}

impl FileFetch {
    /// A policy with the default timeout of 30 seconds.
    pub fn new(hosts: Vec<String>, max_bytes: u64) -> Self {
        FileFetch {
            hosts,
            max_bytes,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Whether `url` may be fetched under `hosts`: `http` or `https`, no credentials, and its host (and
/// port, which defaults to the scheme's) is an entry of the list.
pub(crate) fn allowed(hosts: &[String], url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let Some(port) = url.port_or_known_default() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let default = match url.scheme() {
        "http" => 80,
        _ => 443,
    };
    hosts.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        match entry.rsplit_once(':') {
            Some((name, entry_port)) if !entry_port.contains(']') => {
                name == host && entry_port.parse::<u16>().is_ok_and(|p| p == port)
            }
            _ => entry == host && port == default,
        }
    })
}

/// The file name a URL's path ends in, cleaned, if it has one.
fn name_of(url: &Url) -> Option<String> {
    let last = url.path_segments()?.next_back()?;
    let decoded = percent_encoding::percent_decode_str(last).decode_utf8_lossy();
    let name = decoded.trim();
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_owned())
}

/// Reads allowed `url` parts. Cheap to clone (a client is a handle).
#[derive(Debug, Clone)]
pub(crate) struct Fetcher {
    http: reqwest::Client,
    policy: FileFetch,
}

impl Fetcher {
    pub(crate) fn new(policy: FileFetch, use_system_proxy: bool) -> Result<Self, reqwest::Error> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(policy.timeout);
        if !use_system_proxy {
            builder = builder.no_proxy();
        }
        Ok(Fetcher {
            http: builder.build()?,
            policy,
        })
    }

    /// The envelope, with a link on an allowed host turned into the file it names. Anything else
    /// is returned as it came. A fetch that fails is a file that was not kept (the artifact's
    /// entry and an error, never a silent loss).
    pub(crate) async fn resolve(&self, mut env: AgentEnvelope) -> AgentEnvelope {
        let Some(AgentUpdate::Artifact {
            name,
            mime_type,
            uri: Some(uri),
            text: None,
        }) = &env.update
        else {
            return env;
        };
        let Ok(url) = Url::parse(uri) else {
            return env;
        };
        if !allowed(&self.policy.hosts, &url) {
            return env;
        }
        let name = name.clone();
        let declared = mime_type.clone();
        env.update = Some(match self.fetch(&url).await {
            Ok((bytes, served)) => AgentUpdate::File {
                name,
                media_type: declared.or(served),
                filename: name_of(&url),
                bytes,
            },
            Err(reason) => AgentUpdate::FileRefused {
                name,
                mime_type: declared,
                reason,
            },
        });
        env
    }

    pub(crate) async fn resolve_snapshot(&self, mut snap: TaskSnapshot) -> TaskSnapshot {
        let mut resolved = Vec::with_capacity(snap.envelopes.len());
        for env in std::mem::take(&mut snap.envelopes) {
            resolved.push(self.resolve(env).await);
        }
        snap.envelopes = resolved;
        snap
    }

    /// The body, and the media type the server gave it. Nothing of the URL is logged but the host.
    async fn fetch(&self, url: &Url) -> Result<(Vec<u8>, Option<String>), FileRefusal> {
        let host = url.host_str().unwrap_or("?").to_owned();
        let mut response = self.http.get(url.as_str()).send().await.map_err(|e| {
            tracing::warn!(%host, error = %e.without_url(), "a file an agent pointed at could not be fetched");
            FileRefusal::NotKept
        })?;
        let status = response.status();
        if !status.is_success() {
            tracing::warn!(%host, %status, "a file an agent pointed at was refused by its host");
            return Err(FileRefusal::NotKept);
        }
        if response
            .content_length()
            .is_some_and(|len| len > self.policy.max_bytes)
        {
            return Err(FileRefusal::TooLarge);
        }
        let served = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty());
        let mut body: Vec<u8> = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(piece)) => {
                    if body.len() as u64 + piece.len() as u64 > self.policy.max_bytes {
                        return Err(FileRefusal::TooLarge);
                    }
                    body.extend_from_slice(&piece);
                }
                Ok(None) => return Ok((body, served)),
                Err(e) => {
                    tracing::warn!(%host, error = %e.without_url(), "a file an agent pointed at could not be read");
                    return Err(FileRefusal::NotKept);
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn ok(hosts: &[&str], url: &str) -> bool {
        let hosts: Vec<String> = hosts.iter().map(|h| (*h).to_owned()).collect();
        allowed(&hosts, &Url::parse(url).unwrap())
    }

    #[test]
    fn nothing_is_allowed_by_default() {
        assert!(!ok(&[], "https://files.example.com/a.png"));
    }

    #[test]
    fn a_host_is_matched_whole_and_in_lower_case() {
        let list = ["Files.Example.com"];
        assert!(ok(&list, "https://files.example.com/a.png"));
        assert!(ok(&list, "https://FILES.example.com/a.png"));
        assert!(!ok(&list, "https://example.com/a.png"));
        assert!(!ok(&list, "https://evil.files.example.com/a.png"));
        assert!(!ok(&list, "https://files.example.com.evil.net/a.png"));
        assert!(!ok(&list, "https://notfiles.example.com/a.png"));
    }

    #[test]
    fn a_port_is_part_of_the_entry_and_the_default_is_the_schemes() {
        assert!(ok(&["h.example.com"], "https://h.example.com:443/a"));
        assert!(!ok(&["h.example.com"], "https://h.example.com:8443/a"));
        assert!(ok(&["h.example.com"], "http://h.example.com/a"));
        assert!(!ok(&["h.example.com"], "http://h.example.com:8080/a"));
        assert!(ok(&["h.example.com:8080"], "http://h.example.com:8080/a"));
        assert!(!ok(&["h.example.com:8080"], "http://h.example.com/a"));
        assert!(ok(&["10.0.0.5:8080"], "http://10.0.0.5:8080/a"));
        assert!(ok(&["[::1]:9000"], "http://[::1]:9000/a"));
    }

    #[test]
    fn only_http_and_https_without_credentials() {
        let list = ["h.example.com"];
        assert!(!ok(&list, "ftp://h.example.com/a"));
        assert!(!ok(&list, "file://h.example.com/a"));
        assert!(!ok(&list, "https://user:pw@h.example.com/a"));
        assert!(!ok(&list, "https://user@h.example.com/a"));
        // the userinfo trick: the host is what follows the @
        assert!(!ok(&list, "https://h.example.com@evil.net/a"));
    }

    #[test]
    fn a_file_name_comes_from_the_last_path_segment() {
        let name = |u: &str| name_of(&Url::parse(u).unwrap());
        assert_eq!(
            name("https://h/x/chart%20one.png?sig=1").as_deref(),
            Some("chart one.png")
        );
        assert_eq!(name("https://h/").as_deref(), None);
        assert_eq!(name("https://h/dir/").as_deref(), None);
        assert_eq!(name("https://h/a/..").as_deref(), None);
    }
}
