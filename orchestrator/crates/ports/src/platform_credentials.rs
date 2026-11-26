//! What the orchestrator presents to the platform's API: the credentials port of the
//! [`AgentProvisioner`](crate::AgentProvisioner)'s adapter.
//!
//! The adapter asks [`PlatformCredentials::token`] before **every** request and never keeps the
//! answer: a token that expires, rotates or is re-issued is the port's business, so no token is a
//! static string in the adapter's code. The default adapters are in `orch-provisioner-platform`
//! (a fixed token, and an OAuth client-credentials client for a development issuer).

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// The future of [`PlatformCredentials::token`].
pub type PlatformTokenFuture<'a> =
    Pin<Box<dyn Future<Output = Result<PlatformToken, PlatformCredentialsError>> + Send + 'a>>;

/// A bearer token for the platform. Its `Debug` says nothing of it and its memory is wiped on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PlatformToken(String);

impl PlatformToken {
    /// Wraps a token (the text after `Bearer `).
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token, for the one place that puts it in a header.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PlatformToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlatformToken(<redacted>)")
    }
}

/// Why no token could be had. Neither case holds a token, a secret or a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PlatformCredentialsError {
    /// The issuer could not be reached or answered something unusable: try again later.
    #[error("the platform's token issuer is unavailable")]
    Unavailable,
    /// The issuer refused the orchestrator's own credentials: a configuration fault, not a
    /// transient one.
    #[error("the platform's token issuer refused the orchestrator's credentials")]
    Refused,
}

/// What to present to the platform's API.
pub trait PlatformCredentials: Send + Sync + 'static {
    /// A token that is good now. Called before each request.
    fn token(&self) -> PlatformTokenFuture<'_>;
}
