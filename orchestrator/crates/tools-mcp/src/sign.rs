//! Signing the requests to a server that takes signed requests (`docs/api/signed-tool-requests-v1.md`, ADR 0067).
//!
//! A server whose endpoint has [`signing`](orch_ports::ToolServerEndpoint::signing) is sent **no bearer**: each HTTP request
//! carries the header `Tool-Request-Claims`, made just before the bytes leave, over those very bytes, for the thread the call
//! is made for and the thread's owner. Everything about a request that can be wrong is checked when the session opens
//! ([`Signing::of`]), so that a request that cannot be signed is never sent, and not as an unsigned one.

use std::sync::Arc;

use orch_ports::{ToolServerEndpoint, ToolServerError};
use orch_signed_request::{HEADER, Method, Request, Signer, SigningKey};
use reqwest::header::{HeaderName, HeaderValue};
use uuid::Uuid;

/// The signer of one session: its key, and for whom it signs.
#[derive(Debug)]
pub(crate) struct Signing {
    signer: Signer,
    thread: Uuid,
    owner: String,
}

/// Why a request could not be signed, once the session was open. The causes are the clock and the random source: the rest was
/// checked before the session began.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SigningFailed;

impl Signing {
    /// The signing for `server`'s session at `url`, or `None` for a server that is not signed.
    ///
    /// # Errors
    /// [`ToolServerError::Misconfigured`], with nothing sent, when the server is signed and: it also has a bearer, or sets the
    /// header itself, or the request has no thread to be made for, or the key does not parse, or the audience or lifetime are
    /// out of bounds, or the URL has a query or a path the contract cannot carry, or the owner is too long. The text holds no
    /// key.
    pub(crate) fn of(
        server: &ToolServerEndpoint,
        url: &str,
    ) -> Result<Option<Arc<Self>>, ToolServerError> {
        let Some(signing) = &server.signing else {
            return Ok(None);
        };
        let misconfigured = |what: String| ToolServerError::Misconfigured(what);
        if server.bearer.is_some() {
            return Err(misconfigured(
                "a server that takes signed requests takes no bearer".to_owned(),
            ));
        }
        if server
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(HEADER))
        {
            return Err(misconfigured(format!(
                "the header {HEADER} is the one the signed requests carry"
            )));
        }
        let Some(acting) = &server.acting_for else {
            return Err(misconfigured(
                "a signed request is made for a thread and its owner, and has neither".to_owned(),
            ));
        };
        let key = SigningKey::from_pkcs8_pem(signing.key.expose())
            .map_err(|e| misconfigured(e.to_string()))?;
        let signer = Signer::new(key, &signing.audience, signing.ttl_secs)
            .map_err(|e| misconfigured(e.to_string()))?;
        let this = Signing {
            signer,
            thread: acting.thread.0,
            owner: acting.owner.as_str().to_owned(),
        };
        let path = path_of(url).ok_or_else(|| {
            misconfigured("a signed server's URL has a query, or no path".to_owned())
        })?;
        Signer::check(&this.request(Method::Post, &path, b""))
            .map_err(|e| misconfigured(e.to_string()))?;
        Ok(Some(Arc::new(this)))
    }

    fn request<'a>(&'a self, method: Method, path: &'a str, body: &'a [u8]) -> Request<'a> {
        Request {
            method,
            path,
            body,
            thread: self.thread,
            owner: &self.owner,
        }
    }

    /// The header for a request to `uri` with `body`: signed now, with a `cti` of its own.
    pub(crate) fn header(
        &self,
        method: Method,
        uri: &str,
        body: &[u8],
    ) -> Result<(HeaderName, HeaderValue), SigningFailed> {
        let path = path_of(uri).ok_or(SigningFailed)?;
        let iat = u64::try_from(jiff::Timestamp::now().as_second()).map_err(|_| SigningFailed)?;
        let value = self
            .signer
            .sign(&self.request(method, &path, body), iat)
            .map_err(|_| SigningFailed)?;
        let mut value = HeaderValue::from_str(&value).map_err(|_| SigningFailed)?;
        // a credential for one request: never in a log line
        value.set_sensitive(true);
        Ok((HeaderName::from_static("tool-request-claims"), value))
    }
}

/// The path of the request target of `uri`, as it is sent. `None` for a URL with a query, which the contract never signs.
fn path_of(uri: &str) -> Option<String> {
    let url = reqwest::Url::parse(uri).ok()?;
    if url.query().is_some() {
        return None;
    }
    Some(url.path().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_is_the_one_the_request_line_will_carry() {
        assert_eq!(path_of("https://tools.example").as_deref(), Some("/"));
        assert_eq!(
            path_of("https://tools.example/mcp").as_deref(),
            Some("/mcp")
        );
        assert_eq!(
            path_of("http://127.0.0.1:9/a%20b/c").as_deref(),
            Some("/a%20b/c")
        );
        assert_eq!(path_of("https://tools.example/mcp?x=1"), None);
        assert_eq!(path_of("not a url"), None);
    }

    #[test]
    fn the_header_name_is_the_contract_s() {
        assert!(
            HeaderName::from_static("tool-request-claims")
                .as_str()
                .eq_ignore_ascii_case(HEADER)
        );
    }
}
