//! Discovery: from a server's URL to the authorization server that protects it (ADR 0046;
//! MCP authorization specification 2025-11-25, "Authorization Server Discovery").
//!
//! 1. **Protected resource metadata** (RFC 9728): `GET` at the well-known URI built from the server's
//!    URL, the path-inserted form first (`/.well-known/oauth-protected-resource/<path>`), then the
//!    root form. Its `resource` must identify the same URL as the definition's, else the document is
//!    not used (RFC 9728, section 3.3). Its `authorization_servers` are the issuers.
//! 2. **Authorization server metadata** (RFC 8414, and OpenID Connect Discovery for the issuers
//!    that only publish that): for an issuer with a path, `oauth-authorization-server` with the path
//!    inserted, `openid-configuration` with the path inserted, then `openid-configuration` with the
//!    path appended; for one without, `oauth-authorization-server`, then `openid-configuration`. Its
//!    `issuer` must be the issuer asked about (RFC 8414, section 3.3), and
//!    `code_challenge_methods_supported` must list `S256`, else the server is refused.
//!
//! Every URL fetched here goes through the egress guard ([`Http`]); the endpoints the metadata names
//! are judged again when they are used.

use serde::Deserialize;
use url::Url;

use crate::http::{FetchError, Http, Response};

/// The most issuers of a resource that are tried.
const MAX_ISSUERS: usize = 3;

/// The most scopes read from `scopes_supported`.
const MAX_SCOPES: usize = 32;

/// What discovery found: enough to register a client and build the authorization request.
pub(crate) struct Found {
    /// The resource identifier to send as `resource` (RFC 8707): the one the server declares for
    /// itself, which `same_resource` showed is the definition's URL.
    pub(crate) resource: String,
    /// `scopes_supported` of the resource, for a server whose definition names no scope.
    pub(crate) scopes_supported: Vec<String>,
    pub(crate) server: AuthorizationServer,
}

/// The endpoints and capabilities of an authorization server, from its metadata.
pub(crate) struct AuthorizationServer {
    /// Where the person is sent.
    pub(crate) authorization_endpoint: Url,
    pub(crate) token_endpoint: String,
    pub(crate) registration_endpoint: Option<String>,
    pub(crate) revocation_endpoint: Option<String>,
    /// `client_id_metadata_document_supported`.
    pub(crate) client_id_metadata_document: bool,
}

/// Why discovery found nothing usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DiscoveryError {
    /// A request failed or was refused by the egress guard.
    #[error("a request of the discovery failed")]
    Fetch(#[source] FetchError),
    /// No metadata where metadata is published.
    #[error("no metadata was found")]
    NotFound,
    /// Metadata that does not describe what was asked about, or is not usable.
    #[error("the metadata is not usable")]
    Invalid,
    /// The authorization server does not list `S256` among its PKCE methods.
    #[error("the authorization server does not support PKCE with S256")]
    NoPkce,
}

#[derive(Deserialize)]
struct ResourceMetadata {
    resource: String,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Deserialize)]
struct ServerMetadata {
    issuer: String,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    registration_endpoint: Option<String>,
    revocation_endpoint: Option<String>,
    code_challenge_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    client_id_metadata_document_supported: bool,
}

/// Finds the authorization server of the server at `server_url` (a definition's normal-form URL).
pub(crate) async fn discover(http: &Http, server_url: &str) -> Result<Found, DiscoveryError> {
    let resource = Url::parse(server_url).map_err(|_| DiscoveryError::Invalid)?;
    let metadata = fetch_resource_metadata(http, &resource).await?;
    if !same_resource(&metadata.resource, server_url) {
        return Err(DiscoveryError::Invalid);
    }
    let mut last = DiscoveryError::NotFound;
    for issuer in metadata.authorization_servers.iter().take(MAX_ISSUERS) {
        match fetch_server_metadata(http, issuer).await {
            Ok(server) => {
                return Ok(Found {
                    resource: metadata.resource,
                    scopes_supported: metadata
                        .scopes_supported
                        .into_iter()
                        .take(MAX_SCOPES)
                        .collect(),
                    server,
                });
            }
            // a refusal, a failing host and a server without PKCE end the search: the next
            // issuer is not a way around them
            Err(e @ (DiscoveryError::Fetch(_) | DiscoveryError::NoPkce)) => return Err(e),
            Err(e @ (DiscoveryError::NotFound | DiscoveryError::Invalid)) => last = e,
        }
    }
    Err(last)
}

async fn get_json(http: &Http, url: &str) -> Result<Option<Response>, DiscoveryError> {
    match http.get(url).await {
        Ok(response) if response.status == 200 => Ok(Some(response)),
        Ok(_) => Ok(None),
        Err(e) => Err(DiscoveryError::Fetch(e)),
    }
}

async fn fetch_resource_metadata(
    http: &Http,
    resource: &Url,
) -> Result<ResourceMetadata, DiscoveryError> {
    for url in resource_metadata_urls(resource) {
        let Some(response) = get_json(http, &url).await? else {
            continue;
        };
        if let Ok(metadata) = serde_json::from_slice::<ResourceMetadata>(&response.body) {
            return Ok(metadata);
        }
    }
    Err(DiscoveryError::NotFound)
}

async fn fetch_server_metadata(
    http: &Http,
    issuer: &str,
) -> Result<AuthorizationServer, DiscoveryError> {
    let issuer_url = Url::parse(issuer).map_err(|_| DiscoveryError::Invalid)?;
    let mut last = DiscoveryError::NotFound;
    for url in server_metadata_urls(&issuer_url) {
        let Some(response) = get_json(http, &url).await? else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<ServerMetadata>(&response.body) else {
            last = DiscoveryError::Invalid;
            continue;
        };
        // RFC 8414, section 3.3: the issuer in the document is the one it was asked for
        if metadata.issuer != issuer {
            last = DiscoveryError::Invalid;
            continue;
        }
        return found_in(http, metadata);
    }
    Err(last)
}

fn found_in(http: &Http, metadata: ServerMetadata) -> Result<AuthorizationServer, DiscoveryError> {
    // the MCP specification: no `code_challenge_methods_supported` means no PKCE, and the client
    // must refuse to proceed
    let s256 = metadata
        .code_challenge_methods_supported
        .as_deref()
        .is_some_and(|methods| methods.iter().any(|m| m == "S256"));
    if !s256 {
        return Err(DiscoveryError::NoPkce);
    }
    let (Some(authorization), Some(token)) =
        (metadata.authorization_endpoint, metadata.token_endpoint)
    else {
        return Err(DiscoveryError::Invalid);
    };
    // the token endpoint is fetched later, and judged then; judging it now fails the flow before
    // the person is sent anywhere
    http.admit(&token).map_err(DiscoveryError::Fetch)?;
    // the authorization endpoint is only a place to send the person: it may carry a query, which
    // the guard refuses for a fetched URL, so the rest of it is judged
    let mut authorization = Url::parse(&authorization).map_err(|_| DiscoveryError::Invalid)?;
    let mut shape = authorization.clone();
    shape.set_query(None);
    http.admit_shape(shape.as_str())
        .map_err(DiscoveryError::Fetch)?;
    authorization.set_fragment(None);
    Ok(AuthorizationServer {
        authorization_endpoint: authorization,
        token_endpoint: token,
        registration_endpoint: metadata.registration_endpoint,
        revocation_endpoint: metadata.revocation_endpoint,
        client_id_metadata_document: metadata.client_id_metadata_document_supported,
    })
}

/// `scheme://host[:port]` of a URL.
fn origin_of(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// The well-known URLs of the resource metadata of `resource`, in the order they are tried
/// (RFC 9728, section 3.1; MCP authorization, "Protected Resource Metadata Discovery Requirements").
pub(crate) fn resource_metadata_urls(resource: &Url) -> Vec<String> {
    let origin = origin_of(resource);
    let root = format!("{origin}/.well-known/oauth-protected-resource");
    match resource.path() {
        "" | "/" => vec![root],
        path => vec![format!("{root}{path}"), root],
    }
}

/// The well-known URLs of the metadata of the authorization server `issuer`, in the order they are
/// tried (RFC 8414, section 3.1; MCP authorization, "Authorization Server Metadata Discovery").
pub(crate) fn server_metadata_urls(issuer: &Url) -> Vec<String> {
    let origin = origin_of(issuer);
    // a terminating slash is removed before anything is inserted (RFC 8414, section 3)
    let path = issuer.path().trim_end_matches('/');
    if path.is_empty() {
        return vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ];
    }
    vec![
        format!("{origin}/.well-known/oauth-authorization-server{path}"),
        format!("{origin}/.well-known/openid-configuration{path}"),
        format!("{origin}{path}/.well-known/openid-configuration"),
    ]
}

/// Whether `declared` (the `resource` of a metadata document) and `url` (the definition's URL)
/// name the same resource: same scheme, host and port, and the same path, with `https://h` and
/// `https://h/` the same (the MCP specification prefers the first, the URL parser writes the
/// second). A query or a fragment is a different resource.
pub(crate) fn same_resource(declared: &str, url: &str) -> bool {
    let (Ok(a), Ok(b)) = (Url::parse(declared), Url::parse(url)) else {
        return false;
    };
    let path = |u: &Url| match u.path() {
        "" => "/".to_owned(),
        p => p.to_owned(),
    };
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
        && path(&a) == path(&b)
        && a.query().is_none()
        && b.query().is_none()
        && a.fragment().is_none()
        && b.fragment().is_none()
        && a.username().is_empty()
        && a.password().is_none()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn urls(f: fn(&Url) -> Vec<String>, text: &str) -> Vec<String> {
        f(&Url::parse(text).unwrap())
    }

    #[test]
    fn the_resource_metadata_of_a_path_is_inserted_then_the_root() {
        // the example of the MCP specification
        assert_eq!(
            urls(resource_metadata_urls, "https://example.com/public/mcp"),
            [
                "https://example.com/.well-known/oauth-protected-resource/public/mcp",
                "https://example.com/.well-known/oauth-protected-resource",
            ]
        );
        assert_eq!(
            urls(resource_metadata_urls, "https://example.com/"),
            ["https://example.com/.well-known/oauth-protected-resource"]
        );
        assert_eq!(
            urls(resource_metadata_urls, "http://127.0.0.1:8080/mcp"),
            [
                "http://127.0.0.1:8080/.well-known/oauth-protected-resource/mcp",
                "http://127.0.0.1:8080/.well-known/oauth-protected-resource",
            ]
        );
    }

    #[test]
    fn the_server_metadata_urls_follow_the_mcp_priority_order() {
        // the examples of the MCP specification
        assert_eq!(
            urls(server_metadata_urls, "https://auth.example.com/tenant1"),
            [
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/openid-configuration/tenant1",
                "https://auth.example.com/tenant1/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            urls(server_metadata_urls, "https://auth.example.com"),
            [
                "https://auth.example.com/.well-known/oauth-authorization-server",
                "https://auth.example.com/.well-known/openid-configuration",
            ]
        );
        // a terminating slash is removed first (RFC 8414, section 3)
        assert_eq!(
            urls(server_metadata_urls, "https://auth.example.com/tenant1/"),
            urls(server_metadata_urls, "https://auth.example.com/tenant1")
        );
    }

    #[test]
    fn a_resource_is_the_same_resource_with_or_without_the_root_slash_and_no_other_way() {
        assert!(same_resource(
            "https://mcp.example.com",
            "https://mcp.example.com/"
        ));
        assert!(same_resource(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com/mcp"
        ));
        assert!(same_resource(
            "HTTPS://MCP.example.com/mcp",
            "https://mcp.example.com/mcp"
        ));
        assert!(same_resource(
            "https://mcp.example.com:443/mcp",
            "https://mcp.example.com/mcp"
        ));
        assert!(!same_resource(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com/other"
        ));
        assert!(!same_resource(
            "https://mcp.example.com/mcp/",
            "https://mcp.example.com/mcp"
        ));
        assert!(!same_resource(
            "https://evil.example/mcp",
            "https://mcp.example.com/mcp"
        ));
        assert!(!same_resource(
            "http://mcp.example.com/mcp",
            "https://mcp.example.com/mcp"
        ));
        assert!(!same_resource(
            "https://mcp.example.com:8443/mcp",
            "https://mcp.example.com/mcp"
        ));
        assert!(!same_resource(
            "https://mcp.example.com/mcp?a=1",
            "https://mcp.example.com/mcp"
        ));
        assert!(!same_resource(
            "https://mcp.example.com/mcp#f",
            "https://mcp.example.com/mcp"
        ));
        assert!(!same_resource("not a url", "https://mcp.example.com/mcp"));
    }
}
