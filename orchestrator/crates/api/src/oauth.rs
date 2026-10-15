//! The two public routes of an OAuth authorization of a person's own MCP server (ADR 0046, slice 4c):
//! the **callback** the person's browser comes back to, and the **Client ID Metadata Document** the
//! authorization server fetches.
//!
//! Both sit **outside the identity layer**, like the public link of a shared thread (ADR 0040): the
//! browser that reaches the callback is the system browser on the desktop and the in-app tab on
//! mobile, and carries neither the app's bearer token nor the web's cookie (ADR 0047), and the
//! authorization server that fetches the document is no person at all. So no handler here reads
//! `Authorization`, `X-Auth-Request-Email` or a cookie, and a request is answered whether or not it
//! carries one. Both are behind the rate limit of the public routes ([`guard`]).
//!
//! # The callback, `GET /api/user-tool-servers/oauth/callback`
//!
//! Reads `code`, `state`, `iss` and `error` from the query and nothing else (a parameter it does not
//! know is ignored, as RFC 6749 asks of a client; `error_description` is never read). A known
//! parameter given twice makes the request invalid (RFC 6749, 3.1: "MUST NOT be included more than
//! once"), and nothing is spent. What it does with them is [`orch_app::App::finish_user_tool_authorization`],
//! **in a task of its own**: a browser that closes the connection cannot leave it between the broker
//! keeping the grant and the definition naming it.
//!
//! The answer is a small HTML page, **fixed text with nothing of the request in it** (no owner,
//! server, URL, code, state, `iss` or `error`), and these headers:
//!
//! | Header | Value | Why |
//! |---|---|---|
//! | `Content-Security-Policy` | `default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'` | no script, no style, no image and no frame: the page is words |
//! | `Referrer-Policy` | `no-referrer` | a link on the page, or a later navigation, never carries this URL (the `code` and the `state` are in it) |
//! | `Cache-Control` | `no-store` | the URL and the page are never kept |
//! | `X-Content-Type-Options` | `nosniff` | |
//! | `X-Frame-Options` | `DENY` | for the browsers that read it before the CSP |
//! | `X-Robots-Tag` | `noindex, nofollow` | |
//!
//! `HEAD` is **not** answered as `GET` (axum would run the handler and drop the body): a prefetch
//! that asked only for the headers would spend the `state`. It is a 405.
//!
//! The query is **never logged**: [`crate::trace::redact_uri`] writes `…` for it on this path.
//!
//! # The document, `GET <path of userToolServers.oauth.clientMetadataUrl>`
//!
//! The orchestrator's own client, as the draft of OAuth Client ID Metadata Documents describes it:
//! `client_id` (the URL, byte for byte as the broker sends it), `client_name`, `redirect_uris` (the
//! callback above and nothing else: the browser always comes back to the orchestrator, ADR 0047),
//! `grant_types`, `response_types` and `token_endpoint_auth_method: none` (a public client with
//! PKCE). It holds nothing secret and is built once, at startup.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_app::{AuthorizationOutcome, OAuthCallback};
use orch_ports::{Ports, ToolSecret};
use serde_json::json;
use url::Url;

use crate::ApiState;
use crate::limiter::{LinkKey, PublicLimiter};
use crate::problem::Problem;

/// The path of the callback.
pub const CALLBACK_PATH: &str = "/api/user-tool-servers/oauth/callback";

/// The key the callback is limited under, beside the share links' tokens. The document has a key of
/// its own ([`DOCUMENT_LIMIT_KEY`]): one anonymous client that floods one route must not push the
/// other, and every callback of everybody, into 429.
const CALLBACK_LIMIT_KEY: &str = "oauth-callback";

/// The key the document is limited under.
const DOCUMENT_LIMIT_KEY: &str = "oauth-client-metadata";

/// The paths the orchestrator serves, or its edge or the web serve at the same origin: the document is
/// not served under any of them (`orch-config`'s rule reads the same list, and a test of the binary
/// holds the two together, and holds both to the paths the edges and the web name).
pub const RESERVED_PATHS: [&str; 25] = [
    // the orchestrator, and oauth2-proxy's own routes
    "/api",
    "/agui",
    "/mcp",
    "/thread-tools",
    "/webhooks",
    "/healthz",
    "/readyz",
    "/metrics",
    "/oauth2",
    // the edge's own (`/edge-healthz`) and the well-known documents the web serves (the mobile app
    // links)
    "/edge-healthz",
    "/.well-known",
    // the web: its pages, its runtime configuration, its files and the static export's own
    "/config.json",
    "/s",
    "/auth",
    "/signed-in",
    "/threads",
    "/_next",
    "/_not-found",
    "/brand",
    "/404.html",
    "/index.html",
    "/favicon.ico",
    "/icon.svg",
    "/apple-icon.png",
    "/manifest.webmanifest",
];

/// The longest query the callback reads, in bytes. A `code` may be a long signed token, but nothing
/// that is not the `code` has any use for kilobytes.
const MAX_QUERY_BYTES: usize = 16 * 1024;

/// How long an authorization server may keep the document (the draft leaves it to HTTP caching): five
/// minutes. The document is static, but a change of the client's name or redirect (a new deployment)
/// should not be hidden for an hour from a server that will show the name on a consent page.
const DOCUMENT_CACHE_CONTROL: &str = "public, max-age=300";

// ---------------------------------------------------------------------------------- the guards

/// The rate limit of the callback: the limiter of the public routes (ADR 0040, section 10), under a
/// key of its own, so that the callback is not charged to a link and a link not to the callback; all
/// public routes draw on the bucket of all of them together. With no limiter (the composition did not
/// build one) the route is served unlimited: the callback does one indexed delete of a hash, and what
/// it does after that needs a state only a signed-in person's authorization made.
///
/// A request that is refused is answered with **the page** (HTTP 429, `Retry-After`, the headers of
/// every answer of the callback), not a problem: the browser is a person's. The link was not spent: a
/// limited request never reaches the handler.
pub(crate) async fn callback_guard(
    State(limiter): State<Option<Arc<PublicLimiter>>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(wait) = refused(limiter.as_deref(), CALLBACK_LIMIT_KEY) {
        let mut response = html_page(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests",
            "Too many requests",
            "Too many requests were made to this address. Wait a moment and reload this page; \
             if it still does not work, go back to the app and start the authorization again.",
        );
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(wait));
        return response;
    }
    next.run(request).await
}

/// The rate limit of the document, under its own key. A refused request is a problem with
/// `Retry-After`: the fetcher is an authorization server.
pub(crate) async fn document_guard(
    State(limiter): State<Option<Arc<PublicLimiter>>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(wait) = refused(limiter.as_deref(), DOCUMENT_LIMIT_KEY) {
        let mut response = Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many requests: try again shortly",
        )
        .into_response();
        let headers = response.headers_mut();
        headers.insert(header::RETRY_AFTER, HeaderValue::from(wait));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return response;
    }
    next.run(request).await
}

/// How many seconds to wait, when `key` has no room.
fn refused(limiter: Option<&PublicLimiter>, key: &str) -> Option<u64> {
    limiter?
        .admit(LinkKey::of(key))
        .err()
        .map(|limited| limited.retry_after_secs)
}

/// The timeout layer of the API ends a callback that takes too long with a bare `503`: on this route
/// that is the page of "could not be finished", with the headers of every other answer. (The work runs
/// on in its own task, so the person may find the server connected.)
pub(crate) async fn page_for_a_timeout(response: Response) -> Response {
    if response.status() == StatusCode::SERVICE_UNAVAILABLE
        && !response.headers().contains_key(header::CONTENT_TYPE)
    {
        return page(AuthorizationOutcome::Unavailable);
    }
    response
}

// --------------------------------------------------------------------------------- the callback

/// `GET /api/user-tool-servers/oauth/callback`: finishes an authorization with what the browser
/// brought back, and answers a page. See the module's docs.
pub(crate) async fn callback<P: Ports>(State(state): State<ApiState<P>>, uri: Uri) -> Response {
    let outcome = match parse_query(uri.query().unwrap_or_default()) {
        Some(callback) => {
            let app = Arc::clone(&state.app);
            // a task of its own: see the module's docs
            match tokio::spawn(async move { app.finish_user_tool_authorization(callback).await })
                .await
            {
                Ok(outcome) => outcome,
                Err(ended) => {
                    tracing::error!(
                        panicked = ended.is_panic(),
                        "the callback of an OAuth authorization ended before it finished"
                    );
                    AuthorizationOutcome::Unavailable
                }
            }
        }
        None => AuthorizationOutcome::InvalidLink,
    };
    page(outcome)
}

/// `HEAD` on the callback: nothing is run. A prefetch that asked for the headers does not spend a
/// state.
pub(crate) async fn head_is_not_get() -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("GET"));
    harden(response)
}

/// The four parameters of a callback, or `None` for a query that is not one: too long, or a known
/// parameter given twice. A parameter that is not one of the four is ignored.
fn parse_query(query: &str) -> Option<OAuthCallback> {
    if query.len() > MAX_QUERY_BYTES {
        return None;
    }
    let mut callback = OAuthCallback::default();
    let (mut seen_state, mut seen_code, mut seen_iss, mut seen_error) =
        (false, false, false, false);
    for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
        let seen = match name.as_ref() {
            "state" => &mut seen_state,
            "code" => &mut seen_code,
            "iss" => &mut seen_iss,
            "error" => &mut seen_error,
            // `error_description`, `error_uri`, `session_state`, whatever else: not read
            _ => continue,
        };
        if std::mem::replace(seen, true) {
            return None;
        }
        match name.as_ref() {
            "state" => callback.state = Some(value.into_owned()),
            "code" => callback.code = Some(ToolSecret::new(value.into_owned())),
            "iss" => callback.iss = Some(value.into_owned()),
            // the text of the error is a stranger's and is not read: that there is one is all
            _ => callback.error = true,
        }
    }
    Some(callback)
}

/// The headers of the page (see the module's docs).
fn harden(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in [
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.insert(
        HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

/// The page a browser is shown. Every word is fixed here; nothing of the request is in it.
fn page(outcome: AuthorizationOutcome) -> Response {
    let (status, title, heading, text) = match outcome {
        AuthorizationOutcome::Connected => (
            StatusCode::OK,
            "Authorization complete",
            "You can return to the app",
            "The server is connected. You can close this page and go back to the app.",
        ),
        AuthorizationOutcome::InvalidLink => (
            StatusCode::BAD_REQUEST,
            "This link is not valid",
            "This link is not valid",
            "This authorization link has expired, has already been used, or does not match an \
             authorization that is waiting. Go back to the app and start the authorization again.",
        ),
        AuthorizationOutcome::NotCompleted => (
            StatusCode::BAD_REQUEST,
            "Authorization not completed",
            "The authorization was not completed",
            "You may have declined it, the authorization server may have refused it, or the \
             server may have been changed in the meantime. Go back to the app and try again.",
        ),
        AuthorizationOutcome::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Authorization could not be finished",
            "The authorization could not be finished",
            "Something is not available right now. Go back to the app and start the \
             authorization again in a moment.",
        ),
    };
    html_page(status, title, heading, text)
}

/// A page of fixed words with the headers of every answer of the callback.
fn html_page(status: StatusCode, title: &str, heading: &str, text: &str) -> Response {
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"light dark\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>{title}</title>\n</head>\n<body>\n<main>\n<h1>{heading}</h1>\n<p>{text}</p>\n\
         </main>\n</body>\n</html>\n"
    );
    let mut response = (status, html).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    harden(response)
}

// ---------------------------------------------------------------------------------- the document

/// Why a Client ID Metadata Document cannot be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ClientMetadataError {
    /// The client identifier is not an absolute URL.
    #[error("the client metadata URL is not an absolute URL")]
    NotAUrl,
    /// The client identifier is not `https`, has no host, has user information or a fragment, or has
    /// no path of its own (the draft: it "MUST contain a path component").
    #[error(
        "the client metadata URL must be an https URL with a host and a path, and no user information or fragment"
    )]
    Shape,
    /// The document would be served under a path the orchestrator or its edge serves already.
    #[error("the client metadata URL's path is under {0}, which is served already")]
    Reserved(&'static str),
    /// The redirect URI is not an absolute URL.
    #[error("the redirect URI is not an absolute URL")]
    RedirectNotAUrl,
    /// The redirect URI is not at the path the router serves the callback at ([`CALLBACK_PATH`]): the
    /// document would list a redirect that ends at a 404.
    #[error(
        "the redirect URI's path is not {CALLBACK_PATH}, the only path the callback is served at"
    )]
    RedirectPath,
    /// The name is empty or holds a line break.
    #[error("the client name is empty or has a control character")]
    Name,
}

/// The Client ID Metadata Document of this orchestrator, made once: where it is served and what it
/// says. See the module's docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientMetadata {
    path: String,
    body: String,
}

impl ClientMetadata {
    /// The document for the client `client_id_url` named `client_name`, coming back to
    /// `redirect_uri`.
    ///
    /// # Errors
    /// [`ClientMetadataError`].
    pub fn new(
        client_id_url: &str,
        client_name: &str,
        redirect_uri: &str,
    ) -> Result<Self, ClientMetadataError> {
        let url = Url::parse(client_id_url).map_err(|_| ClientMetadataError::NotAUrl)?;
        if url.scheme() != "https"
            || !url.has_host()
            || url.path() == "/"
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ClientMetadataError::Shape);
        }
        let path = url.path().to_owned();
        if let Some(prefix) = RESERVED_PATHS
            .iter()
            .find(|p| path == **p || path.starts_with(&format!("{p}/")))
        {
            return Err(ClientMetadataError::Reserved(prefix));
        }
        let redirect =
            Url::parse(redirect_uri).map_err(|_| ClientMetadataError::RedirectNotAUrl)?;
        if redirect.path() != CALLBACK_PATH {
            return Err(ClientMetadataError::RedirectPath);
        }
        if client_name.trim().is_empty() || client_name.chars().any(char::is_control) {
            return Err(ClientMetadataError::Name);
        }
        let body = json!({
            "client_id": client_id_url,
            "client_name": client_name,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })
        .to_string();
        Ok(ClientMetadata { path, body })
    }

    /// The path the document is served at: the path of the client identifier.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The document, as JSON text.
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// `GET <path>`: the document. Public, cacheable for an hour, never indexed.
pub(crate) async fn client_metadata(State(document): State<Arc<ClientMetadata>>) -> Response {
    let mut response = Response::new(Body::from(document.body.clone()));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(DOCUMENT_CACHE_CONTROL),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A redirect URI the router serves.
    const CALLBACK: &str = "https://chat.example.com/api/user-tool-servers/oauth/callback";

    #[test]
    fn the_four_parameters_are_read_and_the_rest_is_not() {
        let callback = parse_query(
            "code=a%20code&state=s-1&iss=https%3A%2F%2Fas.example.com&error_description=SENTINEL&x=1",
        )
        .unwrap();
        assert_eq!(callback.code.as_ref().unwrap().expose(), "a code");
        assert_eq!(callback.state.as_deref(), Some("s-1"));
        assert_eq!(callback.iss.as_deref(), Some("https://as.example.com"));
        assert!(!callback.error, "`error_description` is not `error`");
        let declined = parse_query("error=access_denied&state=s-1").unwrap();
        assert!(declined.error && declined.code.is_none());
        let empty = parse_query("").unwrap();
        assert!(empty.state.is_none() && empty.code.is_none() && !empty.error);
    }

    #[test]
    fn a_parameter_given_twice_is_not_a_callback() {
        for query in [
            "state=a&state=b",
            "code=a&code=b",
            "iss=a&iss=b",
            "error=a&error=b",
            "code=a&state=s&iss=x&iss=y",
        ] {
            assert!(parse_query(query).is_none(), "{query}");
        }
        // a name that is not one of the four may repeat
        assert!(parse_query("x=1&x=2&state=s").is_some());
    }

    #[test]
    fn an_over_long_query_is_not_a_callback() {
        assert!(parse_query(&format!("code={}", "c".repeat(MAX_QUERY_BYTES))).is_none());
        assert!(parse_query(&format!("code={}", "c".repeat(MAX_QUERY_BYTES - 5))).is_some());
    }

    #[test]
    fn the_document_names_the_client_and_the_callback_and_nothing_else() {
        let document = ClientMetadata::new(
            "https://chat.example.com/oauth/client.json",
            "Acme Chat",
            "https://chat.example.com/api/user-tool-servers/oauth/callback",
        )
        .unwrap();
        assert_eq!(document.path(), "/oauth/client.json");
        let json: serde_json::Value = serde_json::from_str(document.body()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "client_id": "https://chat.example.com/oauth/client.json",
                "client_name": "Acme Chat",
                "redirect_uris": ["https://chat.example.com/api/user-tool-servers/oauth/callback"],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            })
        );
    }

    #[test]
    fn the_client_id_is_the_url_as_written() {
        // the authorization server compares the document's `client_id` with the URL it fetched
        let url = "https://Chat.Example.com:443/oauth/c.json?v=1";
        let document = ClientMetadata::new(url, "n", CALLBACK).unwrap();
        let json: serde_json::Value = serde_json::from_str(document.body()).unwrap();
        assert_eq!(json["client_id"], url);
        assert_eq!(document.path(), "/oauth/c.json");
    }

    #[test]
    fn a_document_that_cannot_be_served_is_refused() {
        let ok = |url: &str| ClientMetadata::new(url, "n", CALLBACK);
        assert_eq!(ok("nope"), Err(ClientMetadataError::NotAUrl));
        for url in [
            "http://chat.example.com/c",
            "https://chat.example.com/",
            "https://chat.example.com",
            "https://chat.example.com/c#f",
            "https://u:p@chat.example.com/c",
        ] {
            assert_eq!(ok(url), Err(ClientMetadataError::Shape), "{url}");
        }
        for path in [
            "/api",
            "/api/x",
            "/agui/x",
            "/mcp",
            "/oauth2/x",
            "/healthz",
            "/edge-healthz",
            "/.well-known/x",
            "/config.json",
            "/s/doc",
            "/_next/static/x.js",
            "/brand/panda.svg",
            "/favicon.ico",
        ] {
            assert!(
                matches!(
                    ok(&format!("https://chat.example.com{path}")),
                    Err(ClientMetadataError::Reserved(_))
                ),
                "{path}"
            );
        }
        assert!(ok("https://chat.example.com/apiary").is_ok());
        assert!(ok("https://chat.example.com/sx/doc").is_ok());
        assert_eq!(
            ClientMetadata::new("https://chat.example.com/c", "n", "not a url"),
            Err(ClientMetadataError::RedirectNotAUrl)
        );
        // the only redirect the document lists is the one the router serves
        for redirect in [
            "https://chat.example.com/cb",
            "https://chat.example.com/",
            "https://chat.example.com/api/user-tool-servers/oauth/callback/",
            "https://chat.example.com/api/user-tool-servers/oauth/callbackx",
        ] {
            assert_eq!(
                ClientMetadata::new("https://chat.example.com/c", "n", redirect),
                Err(ClientMetadataError::RedirectPath),
                "{redirect}"
            );
        }
        for name in ["", "  ", "two\nlines"] {
            assert_eq!(
                ClientMetadata::new("https://chat.example.com/c", name, CALLBACK),
                Err(ClientMetadataError::Name),
                "{name:?}"
            );
        }
    }
}
