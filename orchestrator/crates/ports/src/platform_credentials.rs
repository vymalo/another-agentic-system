//! The `PlatformCredentials` port: what the orchestrator presents to the Platform API.
//!
//! The provisioner's adapter asks this port for a bearer token each time it calls the platform, so
//! a token is never a static string in code and a renewed token is used at once. The development
//! adapter reads one from the mock issuer; a deployment's adapter is another crate behind the same
//! trait.

use std::fmt;
use std::future::Future;

use orch_core::{BoxError, Classify, ErrorClass};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A bearer token for the Platform API. Its `Debug` never shows it and it is wiped when dropped.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PlatformToken(String);

impl PlatformToken {
    /// Keeps `token`; an empty or blank one is refused.
    pub fn new(token: impl Into<String>) -> Result<Self, CredentialsError> {
        let token = token.into();
        if token.trim().is_empty() {
            return Err(CredentialsError::Invalid);
        }
        Ok(PlatformToken(token))
    }

    /// The token. Handle it as a secret: never log it, never put it in an error.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PlatformToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlatformToken(<redacted>)")
    }
}

/// Why no token was given.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CredentialsError {
    /// The issuer could not be reached or answered with a failure that may pass.
    #[error("the token issuer is unavailable")]
    Unavailable {
        /// The driver's error; it never holds a token or a secret.
        #[source]
        source: BoxError,
    },
    /// The issuer refused the orchestrator's client.
    #[error("the token issuer refused the orchestrator")]
    Refused,
    /// The issuer answered no usable token.
    #[error("the token issuer answered no usable token")]
    Invalid,
}

impl CredentialsError {
    /// The issuer is down or answered nonsense.
    pub fn unavailable(source: impl Into<BoxError>) -> Self {
        CredentialsError::Unavailable {
            source: source.into(),
        }
    }
}

impl Classify for CredentialsError {
    fn class(&self) -> ErrorClass {
        match self {
            CredentialsError::Unavailable { .. } => ErrorClass::Transient,
            CredentialsError::Refused => ErrorClass::Unauthenticated,
            CredentialsError::Invalid => ErrorClass::Invalid,
        }
    }
}

/// Gives the bearer token the orchestrator presents to the Platform API.
pub trait PlatformCredentials: Send + Sync + 'static {
    /// A token that is good now. An implementation may cache one until shortly before it expires;
    /// the caller asks again for every call and never keeps the answer.
    fn token(&self) -> impl Future<Output = Result<PlatformToken, CredentialsError>> + Send;
}
