//! [`Authenticator`] over the identity header a proxy in front of the orchestrator sets
//! (`X-Auth-Request-Email`, set by oauth2-proxy after the login): the behaviour the orchestrator
//! had before it was an OAuth2 resource server (ADR 0012), now one implementation of the port
//! (ADR 0033, `auth.mode: proxy_header`).
//!
//! **The header is trustworthy only behind a proxy that strips the copies a client sends.** This
//! authenticator cannot tell the difference: that is why `jwt` is the mode of a deployment, and
//! this one is for a single user on a local machine, and for the one release of migration
//! (`jwt_or_proxy_header`).
//!
//! It reads only the header, never the bearer token. A header that is present and is not an
//! e-mail address is refused, even when a development user is configured; with no header, the
//! development user is the identity when there is one (`auth.devUser`, `AUTH_DEV_USER`), and
//! otherwise the request is refused (fail closed).

use orch_core::UserId;
use orch_ports::{AuthError, Authenticator, Credentials, Principal};

/// The identity header oauth2-proxy sets after the login.
pub const IDENTITY_HEADER: &str = "x-auth-request-email";

/// The proxy-header authenticator. `HeaderAuth::default()` has no development user, so a request
/// without the header is refused.
#[derive(Debug, Clone, Default)]
pub struct HeaderAuth {
    dev_user: Option<UserId>,
}

impl HeaderAuth {
    /// No development user: a request without the header is refused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Serves a request without the header as `user`. Development only.
    #[must_use]
    pub fn with_dev_user(mut self, user: UserId) -> Self {
        self.dev_user = Some(user);
        self
    }

    /// The development user, when there is one.
    pub fn dev_user(&self) -> Option<&UserId> {
        self.dev_user.as_ref()
    }
}

fn principal(user: UserId) -> Principal {
    Principal {
        email: Some(user.as_str().to_owned()),
        ..Principal::of(user)
    }
}

impl Authenticator for HeaderAuth {
    async fn authenticate(&self, credentials: &Credentials<'_>) -> Result<Principal, AuthError> {
        match credentials.identity_header {
            Some(value) => {
                let email = Some(value.trim()).filter(|e| !e.is_empty() && e.contains('@'));
                match email {
                    Some(email) => Ok(principal(UserId::new(email))),
                    None => Err(AuthError::invalid_header("invalid X-Auth-Request-Email")),
                }
            }
            None => match &self.dev_user {
                Some(user) => Ok(principal(user.clone())),
                None => Err(AuthError::Missing),
            },
        }
    }
}
