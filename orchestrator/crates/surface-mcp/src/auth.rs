//! Static bearer tokens (ADR 0019): who a token belongs to, and the check that guards the route.
//! A token is a principal (ADR 0033): a user, and the roles the configuration gives that token.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_api::Problem;
use orch_core::UserId;
use orch_ports::{Principal, Role};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Why a set of tokens cannot be used.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    /// No token at all: the surface would refuse everyone, which is a mistake, not a policy.
    #[error("no token is configured")]
    NoTokens,
    /// A token that is too short to resist guessing. A generator gives 32 random bytes as 43
    /// characters of base64: `openssl rand -base64 32`.
    #[error("the token of {user} is shorter than {MIN_TOKEN_BYTES} bytes")]
    TooShort {
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

/// The shortest token the surface accepts, in bytes. A static bearer token does not expire and
/// nothing slows down a guess, so it must be a random secret, not a word.
pub const MIN_TOKEN_BYTES: usize = 32;

/// The principal a request is authenticated as: what the bearer check puts in the request, and what
/// the tools read. It is the only identity an MCP call has (`X-Auth-Request-Email` is never read).
/// Its roles are the ones the token's entry names; none means the policy's default role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpUser(pub Principal);

struct Entry {
    principal: Principal,
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
                &self
                    .entries
                    .iter()
                    .map(|e| &e.principal.user)
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

fn digest_of(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl TokenTable {
    /// Builds the table from `(user, token)` pairs: tokens that carry no role, so that their
    /// holders have what the policy's default role grants.
    pub fn new(
        tokens: impl IntoIterator<Item = (UserId, SecretString)>,
    ) -> Result<Self, TokenError> {
        Self::with_roles(
            tokens
                .into_iter()
                .map(|(user, token)| (user, BTreeSet::new(), token)),
        )
    }

    /// Builds the table from `(user, roles, token)` triples (ADR 0033: `role` of an entry of the
    /// tokens file). The roles are as an identity provider would spell them, compared exactly to
    /// the names of `auth.roles`.
    pub fn with_roles(
        tokens: impl IntoIterator<Item = (UserId, BTreeSet<Role>, SecretString)>,
    ) -> Result<Self, TokenError> {
        let mut entries: Vec<Entry> = Vec::new();
        for (user, roles, token) in tokens {
            let token = token.expose_secret();
            if token.len() < MIN_TOKEN_BYTES {
                return Err(TokenError::TooShort {
                    user: user.to_string(),
                });
            }
            let digest = digest_of(token);
            if let Some(other) = entries.iter().find(|e| e.digest == digest) {
                return Err(TokenError::Shared {
                    first: other.principal.user.to_string(),
                    second: user.to_string(),
                });
            }
            entries.push(Entry {
                principal: Principal {
                    email: Some(user.to_string()),
                    user,
                    name: None,
                    roles,
                    expires_at: None,
                },
                digest,
            });
        }
        if entries.is_empty() {
            return Err(TokenError::NoTokens);
        }
        Ok(TokenTable { entries })
    }

    /// The principal the token belongs to, or `None` for a token nobody has.
    pub fn authenticate(&self, presented: &str) -> Option<&Principal> {
        let presented = digest_of(presented);
        let mut found = None;
        for entry in &self.entries {
            // No early exit: every entry is compared.
            if bool::from(entry.digest.ct_eq(&presented)) {
                found = Some(&entry.principal);
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
/// (RFC 9110); anything else, a missing header, a header that is not text and **more than one
/// `Authorization` header** are `None`: with two, a proxy and this server could read different
/// ones, so none is trusted.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut all = headers.get_all(header::AUTHORIZATION).iter();
    let value = all.next()?;
    if all.next().is_some() {
        return None;
    }
    let value = value.to_str().ok()?;
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
    let principal = bearer(request.headers()).and_then(|token| tokens.authenticate(token));
    let Some(principal) = principal else {
        return unauthorized();
    };
    // Whatever a client sent, the only identity is the token's.
    request.extensions_mut().insert(McpUser(principal.clone()));
    next.run(request).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const ALICE_TOKEN: &str = "alice-token-0123456789abcdef0123456789";
    const ALICE_NEXT: &str = "alice-next-0123456789abcdef0123456789";
    const BOB_TOKEN: &str = "bob-token-0123456789abcdef012345678901";

    fn secret(s: &str) -> SecretString {
        SecretString::from(s.to_owned())
    }

    fn table() -> TokenTable {
        TokenTable::new([
            (UserId::new("alice@example.com"), secret(ALICE_TOKEN)),
            (UserId::new("alice@example.com"), secret(ALICE_NEXT)),
            (UserId::new("bob@example.com"), secret(BOB_TOKEN)),
        ])
        .unwrap()
    }

    #[test]
    fn a_token_finds_its_user_and_a_stranger_finds_nobody() {
        let t = table();
        assert_eq!(t.len(), 3);
        assert_eq!(
            t.authenticate(BOB_TOKEN).unwrap().user.as_str(),
            "bob@example.com"
        );
        // A rotation: both of alice's tokens work.
        for token in [ALICE_TOKEN, ALICE_NEXT] {
            assert_eq!(
                t.authenticate(token).unwrap().user.as_str(),
                "alice@example.com"
            );
        }
        let (with_space, upper, with_newline) = (
            format!("{ALICE_TOKEN} "),
            ALICE_TOKEN.to_uppercase(),
            format!("{BOB_TOKEN}\n"),
        );
        for wrong in ["", "alice", &with_space, &upper, &with_newline] {
            assert!(t.authenticate(wrong).is_none(), "{wrong:?}");
        }
    }

    #[test]
    fn a_token_carries_the_roles_of_its_entry_and_none_by_default() {
        let t = TokenTable::with_roles([
            (
                UserId::new("alice@example.com"),
                BTreeSet::from([Role::new("admin"), Role::new("ci")]),
                secret(ALICE_TOKEN),
            ),
            (
                UserId::new("bob@example.com"),
                BTreeSet::new(),
                secret(BOB_TOKEN),
            ),
        ])
        .unwrap();
        let alice = t.authenticate(ALICE_TOKEN).unwrap();
        assert_eq!(
            alice.roles.iter().map(Role::as_str).collect::<Vec<_>>(),
            ["admin", "ci"]
        );
        assert!(t.authenticate(BOB_TOKEN).unwrap().roles.is_empty());
        // `new` is a token with no role.
        assert!(table().authenticate(ALICE_TOKEN).unwrap().roles.is_empty());
        // Roles are not part of what a table shows either.
        assert!(!format!("{t:?}").contains("admin"));
    }

    #[test]
    fn unusable_sets_are_refused() {
        assert_eq!(TokenTable::new([]).unwrap_err(), TokenError::NoTokens);
        // Empty, a word, and one byte too few.
        for short in ["", "  ", "password", &"x".repeat(MIN_TOKEN_BYTES - 1)] {
            assert!(
                matches!(
                    TokenTable::new([(UserId::new("a@x.io"), secret(short))]).unwrap_err(),
                    TokenError::TooShort { user } if user == "a@x.io"
                ),
                "{short:?}"
            );
        }
        assert!(
            TokenTable::new([(UserId::new("a@x.io"), secret(&"x".repeat(MIN_TOKEN_BYTES)))])
                .is_ok()
        );
        assert!(matches!(
            TokenTable::new([
                (UserId::new("a@x.io"), secret(ALICE_TOKEN)),
                (UserId::new("b@x.io"), secret(ALICE_TOKEN)),
            ])
            .unwrap_err(),
            TokenError::Shared { first, second } if (first.as_str(), second.as_str()) == ("a@x.io", "b@x.io")
        ));
    }

    #[test]
    fn a_table_never_shows_a_token() {
        let shown = format!("{:?}", table());
        assert!(!shown.contains("0123456789"), "{shown}");
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
        // Two headers: none is trusted, even when both say the same.
        let mut two = with("Bearer abc");
        two.append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc"),
        );
        assert_eq!(bearer(&two), None);
    }
}
