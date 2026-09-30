//! Static bearer tokens (ADR 0019): who a token belongs to, and the check that guards the route.

use std::fmt;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_api::Problem;
use orch_core::UserId;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Why a set of tokens cannot be used.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    /// No token at all: the surface would refuse everyone, which is a mistake, not a policy.
    #[error("no token is configured")]
    NoTokens,
    /// A token with no characters.
    #[error("the token of {user} is empty")]
    Empty {
        /// Whose token.
        user: String,
    },
    /// One token for two users: a request could not tell whose it is.
    #[error("the same token is configured for {first} and for {second}")]
    Shared {
        /// The first user with the token.
        first: String,
        /// The second.
        second: String,
    },
}

/// The user a request is authenticated as: what the bearer check puts in the request, and what
/// the tools read. It is the only identity an MCP call has (`X-Auth-Request-Email` is never read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpUser(pub UserId);

struct Entry {
    user: UserId,
    /// SHA-256 of the token. The token itself is not kept: it is hashed once, at startup.
    digest: [u8; 32],
}

/// The bearer tokens and their users.
///
/// A presented token is hashed and its digest compared, in constant time, with the digest of every
/// configured token (all of them, whatever the outcome), so neither the length nor the content of
/// a token, nor its position in the list, shows in the time a check takes. A user may have several
/// tokens (a rotation); a token belongs to one user.
pub struct TokenTable {
    entries: Vec<Entry>,
}

impl fmt::Debug for TokenTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenTable")
            .field(
                "users",
                &self.entries.iter().map(|e| &e.user).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

fn digest_of(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl TokenTable {
    /// Builds the table from `(user, token)` pairs.
    pub fn new(
        tokens: impl IntoIterator<Item = (UserId, SecretString)>,
    ) -> Result<Self, TokenError> {
        let mut entries: Vec<Entry> = Vec::new();
        for (user, token) in tokens {
            let token = token.expose_secret();
            if token.trim().is_empty() {
                return Err(TokenError::Empty {
                    user: user.to_string(),
                });
            }
            let digest = digest_of(token);
            if let Some(other) = entries.iter().find(|e| e.digest == digest) {
                return Err(TokenError::Shared {
                    first: other.user.to_string(),
                    second: user.to_string(),
                });
            }
            entries.push(Entry { user, digest });
        }
        if entries.is_empty() {
            return Err(TokenError::NoTokens);
        }
        Ok(TokenTable { entries })
    }

    /// The user the token belongs to, or `None` for a token nobody has.
    pub fn authenticate(&self, presented: &str) -> Option<&UserId> {
        let presented = digest_of(presented);
        let mut found = None;
        for entry in &self.entries {
            // No early exit: every entry is compared.
            if bool::from(entry.digest.ct_eq(&presented)) {
                found = Some(&entry.user);
            }
        }
        found
    }

    /// The number of tokens.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Never true: a table has at least one token.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The token of an `Authorization: Bearer <token>` header. The scheme is case-insensitive
/// (RFC 9110); anything else, a missing header and a header that is not text are `None`.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// The response for no bearer token and for one nobody has. It does not say which.
fn unauthorized() -> Response {
    let mut response = Problem::unauthorized("a valid bearer token is required").into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

/// The guard of the MCP route: a request without a known bearer token is 401 with
/// `WWW-Authenticate: Bearer` before anything else looks at it; a known one carries
/// [`McpUser`] into the tools.
pub(crate) async fn require_bearer(
    State(tokens): State<Arc<TokenTable>>,
    mut request: Request,
    next: Next,
) -> Response {
    let user = bearer(request.headers()).and_then(|token| tokens.authenticate(token));
    let Some(user) = user else {
        return unauthorized();
    };
    // Whatever a client sent, the only identity is the token's.
    request.extensions_mut().insert(McpUser(user.clone()));
    next.run(request).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn secret(s: &str) -> SecretString {
        SecretString::from(s.to_owned())
    }

    fn table() -> TokenTable {
        TokenTable::new([
            (UserId::new("alice@example.com"), secret("alice-token")),
            (UserId::new("alice@example.com"), secret("alice-next-token")),
            (UserId::new("bob@example.com"), secret("bob-token")),
        ])
        .unwrap()
    }

    #[test]
    fn a_token_finds_its_user_and_a_stranger_finds_nobody() {
        let t = table();
        assert_eq!(t.len(), 3);
        assert_eq!(
            t.authenticate("bob-token").unwrap().as_str(),
            "bob@example.com"
        );
        // A rotation: both of alice's tokens work.
        for token in ["alice-token", "alice-next-token"] {
            assert_eq!(t.authenticate(token).unwrap().as_str(), "alice@example.com");
        }
        for wrong in ["", "alice", "alice-token ", "ALICE-TOKEN", "bob-token\n"] {
            assert!(t.authenticate(wrong).is_none(), "{wrong:?}");
        }
    }

    #[test]
    fn unusable_sets_are_refused() {
        assert_eq!(TokenTable::new([]).unwrap_err(), TokenError::NoTokens);
        assert!(matches!(
            TokenTable::new([(UserId::new("a@x.io"), secret("  "))]).unwrap_err(),
            TokenError::Empty { user } if user == "a@x.io"
        ));
        assert!(matches!(
            TokenTable::new([
                (UserId::new("a@x.io"), secret("same")),
                (UserId::new("b@x.io"), secret("same")),
            ])
            .unwrap_err(),
            TokenError::Shared { first, second } if (first.as_str(), second.as_str()) == ("a@x.io", "b@x.io")
        ));
    }

    #[test]
    fn a_table_never_shows_a_token() {
        let shown = format!("{:?}", table());
        assert!(!shown.contains("token"), "{shown}");
        assert!(shown.contains("alice@example.com"));
    }

    #[test]
    fn the_bearer_scheme_is_case_insensitive_and_nothing_else_counts() {
        let with = |value: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            h
        };
        assert_eq!(bearer(&with("Bearer abc")), Some("abc"));
        assert_eq!(bearer(&with("bearer abc")), Some("abc"));
        assert_eq!(bearer(&with("BEARER  abc ")), Some("abc"));
        assert_eq!(bearer(&with("Basic abc")), None);
        assert_eq!(bearer(&with("Bearer")), None);
        assert_eq!(bearer(&with("Bearer ")), None);
        assert_eq!(bearer(&with("abc")), None);
        assert_eq!(bearer(&HeaderMap::new()), None);
    }
}
