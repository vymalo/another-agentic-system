//! Asking the issuer for its keys: the discovery document, when no `jwksUrl` is configured, and
//! the JWKS. Both are plain HTTP GETs of JSON.
//!
//! The issuer and the JWKS URL are the operator's own configuration, not an address a caller
//! supplies, and a deployment's identity provider is as often a private address (a Keycloak in
//! the cluster) as a public one, so there is no filter on the address as there would be for a
//! URL a person typed. What the requests do is bounded: a timeout, no redirect (a redirected
//! request would go where the configuration did not name), a body of at most 1 MiB, and the
//! discovery document must say it is the issuer's own.

use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderValue};
use serde_json::Value;
use url::Url;

/// The largest body read from the issuer.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Why the keys could not be fetched. The detail never carries a body, and the URL is the
/// operator's own, so it may be logged (never shown to a caller).
#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    /// The request failed: connection, timeout, a status that is not 200, a redirect.
    #[error("the request to the issuer failed: {0}")]
    Request(String),
    /// The answer is not what was asked for.
    #[error("the issuer's answer is not usable: {0}")]
    Unusable(&'static str),
}

/// Where the JWKS is: configured, or found in the discovery document.
#[derive(Debug, Clone)]
pub(crate) enum Source {
    /// `auth.jwt.jwksUrl`.
    Url(Url),
    /// `{issuer}/.well-known/openid-configuration`, which names the JWKS.
    Discover { issuer: String, document: Url },
}

pub(crate) struct Fetcher {
    http: reqwest::Client,
    source: Source,
}

impl Fetcher {
    pub(crate) fn new(http: reqwest::Client, source: Source) -> Self {
        Fetcher { http, source }
    }

    /// The JWKS document.
    pub(crate) async fn jwks(&self) -> Result<Value, FetchError> {
        let url = match &self.source {
            Source::Url(url) => url.clone(),
            Source::Discover { issuer, document } => self.discover(issuer, document).await?,
        };
        self.get_json(&url).await
    }

    /// The JWKS URL the discovery document names. The document must carry the issuer it was
    /// asked for (OpenID Connect Discovery 1.0, 4.3: a document that says another issuer is a
    /// mix-up), and the JWKS must not be reached over plain HTTP from an HTTPS issuer.
    async fn discover(&self, issuer: &str, document: &Url) -> Result<Url, FetchError> {
        let doc = self.get_json(document).await?;
        if doc.get("issuer").and_then(Value::as_str) != Some(issuer) {
            return Err(FetchError::Unusable(
                "the discovery document names another issuer",
            ));
        }
        let uri = doc
            .get("jwks_uri")
            .and_then(Value::as_str)
            .ok_or(FetchError::Unusable(
                "the discovery document has no jwks_uri",
            ))?;
        jwks_uri(document, uri)
    }

    async fn get_json(&self, url: &Url) -> Result<Value, FetchError> {
        let mut response = self
            .http
            .get(url.clone())
            .header(ACCEPT, HeaderValue::from_static("application/json"))
            .send()
            .await
            .map_err(|e| FetchError::Request(e.without_url().to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let kind = if status.is_redirection() {
                format!("{status} (a redirect is never followed: configure the final URL)")
            } else {
                status.to_string()
            };
            return Err(FetchError::Request(kind));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::Request(e.without_url().to_string()))?
        {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(FetchError::Unusable("the answer is larger than 1 MiB"));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| FetchError::Unusable("the answer is not JSON"))
    }
}

/// The JWKS URL a discovery document at `document` names: an `http(s)` URL without credentials,
/// and not plain HTTP when the issuer is served over HTTPS.
fn jwks_uri(document: &Url, uri: &str) -> Result<Url, FetchError> {
    let jwks = Url::parse(uri).map_err(|_| FetchError::Unusable("the jwks_uri is not a URL"))?;
    if !matches!(jwks.scheme(), "http" | "https")
        || !jwks.has_host()
        || !jwks.username().is_empty()
        || jwks.password().is_some()
    {
        return Err(FetchError::Unusable(
            "the jwks_uri is not an http(s) URL without credentials",
        ));
    }
    if document.scheme() == "https" && jwks.scheme() != "https" {
        return Err(FetchError::Unusable(
            "the jwks_uri downgrades an https issuer to http",
        ));
    }
    Ok(jwks)
}

/// The client every fetch goes through: a timeout, and no redirect.
pub(crate) fn client(
    timeout: Duration,
    use_system_proxy: bool,
) -> Result<reqwest::Client, reqwest::Error> {
    let builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    let builder = if use_system_proxy {
        builder
    } else {
        builder.no_proxy()
    };
    builder.build()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_jwks_uri_is_an_http_url_that_does_not_downgrade_the_issuer() {
        let https = Url::parse("https://idp.example/.well-known/openid-configuration").unwrap();
        let http = Url::parse("http://idp.local/.well-known/openid-configuration").unwrap();
        // Another host is fine (some issuers keep their keys elsewhere); another scheme is not.
        assert!(jwks_uri(&https, "https://keys.example/jwks").is_ok());
        assert!(jwks_uri(&http, "http://idp.local/jwks").is_ok());
        assert!(jwks_uri(&http, "https://idp.local/jwks").is_ok());
        for bad in [
            "http://idp.example/jwks",
            "ftp://idp.example/jwks",
            "file:///etc/passwd",
            "https://user:pass@idp.example/jwks",
            "/jwks",
            "not a url",
            "",
        ] {
            assert!(jwks_uri(&https, bad).is_err(), "{bad}");
        }
        assert!(jwks_uri(&http, "https://user@idp.local/jwks").is_err());
    }
}
