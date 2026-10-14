//! The OAuth requests the broker makes to an authorization server's token, registration and
//! revocation endpoints, and the PKCE and `state` values it makes.
//!
//! What an authorization server answers is **never echoed**: an error response may repeat the code,
//! the verifier or the refresh token that was sent, so the broker keeps of it the HTTP status and,
//! when it is one of the error codes the RFCs define (`invalid_grant`), that code, and nothing
//! else: a text of the server's own choosing is not a code, however well formed.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_ports::ToolSecret;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::http::{BasicAuth, Body, FetchError, Http};
use crate::material::{AuthMethod, ClientRecord, secret_opt};

/// The longest an access token may be said to live: a year. A longer `expires_in` is cut.
const MAX_EXPIRES_IN: u64 = 365 * 24 * 3600;

/// What an access token is assumed to live when the authorization server says nothing and has given
/// a refresh token to ask again with.
pub(crate) const ASSUMED_TTL_SECS: u64 = 3600;

/// `bytes` bytes from the operating system's random source, as base64url without padding.
pub(crate) fn random_token(bytes: usize) -> Option<String> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::fill(&mut buffer).ok()?;
    let text = URL_SAFE_NO_PAD.encode(&buffer);
    buffer.fill(0);
    Some(text)
}

/// A PKCE `code_verifier` (RFC 7636, section 4.1): 32 random bytes, 43 characters of the unreserved
/// set, 256 bits of entropy.
pub(crate) fn new_verifier() -> Option<String> {
    random_token(32)
}

/// The `S256` `code_challenge` of a verifier (RFC 7636, section 4.2):
/// `BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))`, without padding.
pub(crate) fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What the token endpoint issued.
pub(crate) struct Issued {
    pub(crate) access: ToolSecret,
    pub(crate) expires_in: Option<u64>,
    pub(crate) refresh: Option<ToolSecret>,
}

/// Why a token request did not give tokens.
pub(crate) enum TokenFailure {
    /// The authorization server refused the grant (`400` or `401`, or a `200` with an `error`): the
    /// code or the refresh token is no good. Carries the OAuth error code if it is one the RFCs
    /// define, else `unknown` (see [`error_code`]).
    Grant(&'static str),
    /// Anything else: a failing server, an answer that is not a token response, a refused URL.
    Unavailable(&'static str, Option<FetchError>),
}

#[derive(Deserialize)]
struct TokenBody {
    #[serde(default, with = "secret_opt")]
    access_token: Option<ToolSecret>,
    token_type: Option<String>,
    expires_in: Option<Value>,
    #[serde(default, with = "secret_opt")]
    refresh_token: Option<ToolSecret>,
    error: Option<String>,
}

/// The error codes of a token response: RFC 6749, section 5.2, and `invalid_target` of RFC 8707,
/// section 2. The code is written in the log, so a code the server made up is not one: it could be
/// anything, the refresh token among it.
const ERROR_CODES: [&str; 7] = [
    "invalid_request",
    "invalid_client",
    "invalid_grant",
    "unauthorized_client",
    "unsupported_grant_type",
    "invalid_scope",
    "invalid_target",
];

/// `error` of a token response if it is one of [`ERROR_CODES`] (compared exactly, as the RFC
/// writes them), else `unknown`.
pub(crate) fn error_code(text: Option<&str>) -> &'static str {
    text.and_then(|code| ERROR_CODES.iter().find(|known| **known == code))
        .copied()
        .unwrap_or("unknown")
}

/// `expires_in` in whole seconds, cut at a year. **A lifetime of zero is no lifetime**: some servers
/// send `0` for a token that does not expire, and a token that "expires now" would be refreshed by
/// every lease, so it is read as if the server had said nothing (see `tokens_of`).
fn seconds(value: &Value) -> Option<u64> {
    let n = match value {
        Value::Number(n) => n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|f| *f >= 0.0)
                // whole seconds of a number such as 3599.9
                .map(|f| f.floor() as u64)
        }),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }?;
    (n > 0).then(|| n.min(MAX_EXPIRES_IN))
}

/// `POST endpoint` with `fields` and the client's authentication, and the tokens it answers with.
pub(crate) async fn request_token(
    http: &Http,
    endpoint: &str,
    client: &ClientRecord,
    fields: &[(&str, &str)],
) -> Result<Issued, TokenFailure> {
    let response = post_as_client(http, endpoint, client, fields)
        .await
        .map_err(|e| TokenFailure::Unavailable("the token endpoint did not answer", Some(e)))?;

    let parsed = serde_json::from_slice::<TokenBody>(&response.body);
    match response.status {
        200..=299 => {}
        400 | 401 => {
            let code = parsed.ok().and_then(|b| b.error);
            return Err(TokenFailure::Grant(error_code(code.as_deref())));
        }
        _ => {
            return Err(TokenFailure::Unavailable(
                "the token endpoint answered with an error",
                None,
            ));
        }
    }
    let Ok(body) = parsed else {
        return Err(TokenFailure::Unavailable(
            "the token endpoint did not answer with a token response",
            None,
        ));
    };
    let Some(access) = body.access_token else {
        // some servers answer a refused grant with `200` and an `error`
        return Err(match body.error {
            Some(code) => TokenFailure::Grant(error_code(Some(&code))),
            None => TokenFailure::Unavailable(
                "the token endpoint did not answer with a token response",
                None,
            ),
        });
    };
    if body
        .token_type
        .as_deref()
        .is_some_and(|t| !t.eq_ignore_ascii_case("bearer"))
    {
        return Err(TokenFailure::Unavailable(
            "the authorization server issued a token that is not a bearer token",
            None,
        ));
    }
    Ok(Issued {
        access,
        expires_in: body.expires_in.as_ref().and_then(seconds),
        refresh: body.refresh_token,
    })
}

/// A POST to an endpoint of the authorization server that wants the client authenticated: the
/// token and the revocation endpoints.
pub(crate) async fn post_as_client(
    http: &Http,
    endpoint: &str,
    client: &ClientRecord,
    fields: &[(&str, &str)],
) -> Result<crate::http::Response, FetchError> {
    let mut all: Vec<(&str, &str)> = fields.to_vec();
    let mut basic = None;
    match (client.method, client.secret.as_ref()) {
        (AuthMethod::Basic, Some(secret)) => {
            basic = Some(BasicAuth {
                id: &client.id,
                secret,
            });
        }
        (AuthMethod::Post, Some(secret)) => {
            all.push(("client_id", &client.id));
            all.push(("client_secret", secret.expose()));
        }
        // a public client, or a secret that was not kept: identify, authenticate with nothing
        (AuthMethod::None | AuthMethod::Basic | AuthMethod::Post, _) => {
            all.push(("client_id", &client.id));
        }
    }
    http.post(endpoint, Body::form(all), basic).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_s256_challenge_is_the_one_of_rfc_7636_appendix_b() {
        // RFC 7636, appendix B
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_verifier_is_43_unreserved_characters_and_never_repeats() {
        let a = new_verifier().unwrap();
        let b = new_verifier().unwrap();
        assert_eq!(a.len(), 43);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_ne!(a, b);
    }

    #[test]
    fn only_an_error_code_the_rfcs_define_is_repeated() {
        for code in ERROR_CODES {
            assert_eq!(error_code(Some(code)), code);
        }
        // well formed, and not a code: a server can write anything here
        assert_eq!(error_code(Some("tok-9c2f8d")), "unknown");
        assert_eq!(error_code(Some("tok-9c2f8d secret")), "unknown");
        assert_eq!(error_code(Some("invalid_grant ")), "unknown");
        assert_eq!(error_code(Some("INVALID_GRANT")), "unknown");
        assert_eq!(error_code(Some("access_denied")), "unknown");
        assert_eq!(error_code(Some(&"x".repeat(65))), "unknown");
        assert_eq!(error_code(Some("")), "unknown");
        assert_eq!(error_code(None), "unknown");
    }

    #[test]
    fn expires_in_is_read_as_a_number_a_float_or_a_string_and_cut() {
        assert_eq!(seconds(&serde_json::json!(3600)), Some(3600));
        assert_eq!(seconds(&serde_json::json!(3599.9)), Some(3599));
        assert_eq!(seconds(&serde_json::json!("120")), Some(120));
        assert_eq!(seconds(&serde_json::json!(-5)), None);
        // zero is no lifetime: the same as nothing said
        assert_eq!(seconds(&serde_json::json!(0)), None);
        assert_eq!(seconds(&serde_json::json!("0")), None);
        assert_eq!(seconds(&serde_json::json!(0.4)), None);
        assert_eq!(seconds(&serde_json::json!(null)), None);
        assert_eq!(seconds(&serde_json::json!(u64::MAX)), Some(MAX_EXPIRES_IN));
    }
}
