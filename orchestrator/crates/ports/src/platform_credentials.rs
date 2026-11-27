//! What the orchestrator presents to the platform's API: the credentials port of the
//! [`AgentProvisioner`](crate::AgentProvisioner)'s adapter.
//!
//! The adapter asks [`PlatformCredentials::token`] before **every** request and never keeps the
//! answer: a token that expires, rotates or is re-issued is the port's business, so no token is a
//! static string in the adapter's code. The default adapters are in `orch-provisioner-platform`
//! (a fixed token, and an OAuth client-credentials client for a development issuer).
//!
//! A deployment that authenticates by a **client certificate** on the TLS connection (the
//! orchestrator's workload certificate) uses the sibling port [`PlatformClientCertificates`]
//! instead. It is a second, additive port, never a fallback: a provisioner is built with one mode
//! or the other, and a mode that cannot produce its identity sends nothing.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use orch_core::{Classify, ErrorClass};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

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

impl Classify for PlatformCredentialsError {
    fn class(&self) -> ErrorClass {
        match self {
            PlatformCredentialsError::Unavailable => ErrorClass::Transient,
            PlatformCredentialsError::Refused => ErrorClass::Unauthenticated,
        }
    }
}

/// A client-authentication certificate for the platform's TLS listener, with the private key it
/// was issued for. The key is held in memory only, is wiped when the last clone is dropped, and
/// is never shown: `Debug` says the serial number and the end of validity, nothing else.
#[derive(Clone)]
pub struct PlatformClientCertificate {
    chain: Vec<Vec<u8>>,
    key: Arc<Zeroizing<Vec<u8>>>,
    serial: String,
    not_after: jiff::Timestamp,
}

impl PlatformClientCertificate {
    /// Wraps an issued certificate: `chain` is the DER of the leaf first and then the
    /// intermediates that lead to the authority the platform trusts (not the root), `key` the
    /// leaf's private key as PKCS#8 DER, `serial` the leaf's serial number as text and
    /// `not_after` the end of its validity.
    pub fn new(
        chain: Vec<Vec<u8>>,
        key: Zeroizing<Vec<u8>>,
        serial: impl Into<String>,
        not_after: jiff::Timestamp,
    ) -> Self {
        Self {
            chain,
            key: Arc::new(key),
            serial: serial.into(),
            not_after,
        }
    }

    /// The DER of the leaf, then of its intermediates.
    pub fn chain_der(&self) -> &[Vec<u8>] {
        &self.chain
    }

    /// The private key (PKCS#8 DER), for the one place that puts it in a TLS identity.
    pub fn expose_key_pkcs8(&self) -> &[u8] {
        self.key.as_slice()
    }

    /// The leaf's serial number: it changes with every issue, so a client keyed by it is rebuilt
    /// exactly when the certificate is replaced.
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// The end of the leaf's validity.
    pub fn not_after(&self) -> jiff::Timestamp {
        self.not_after
    }
}

impl fmt::Debug for PlatformClientCertificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlatformClientCertificate")
            .field("serial", &self.serial)
            .field("not_after", &self.not_after)
            .finish_non_exhaustive()
    }
}

/// The future of [`PlatformClientCertificates::current`].
pub type PlatformCertificateFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<PlatformClientCertificate, PlatformCredentialsError>>
            + Send
            + 'a,
    >,
>;

/// A client certificate for the platform's TLS listener: the orchestrator's identity when the
/// platform authenticates a workload by the certificate it presents and not by a bearer token.
pub trait PlatformClientCertificates: Send + Sync + 'static {
    /// A certificate that is valid now, with its key. Called before **every** request, so that an
    /// implementation renews it before it expires and the caller keeps nothing of its own.
    ///
    /// An error means there is **no identity**: the caller sends nothing (it never falls back to
    /// another credential and never to an unauthenticated call).
    fn current(&self) -> PlatformCertificateFuture<'_>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_classified_and_the_certificate_shows_no_key() {
        assert_eq!(
            PlatformCredentialsError::Unavailable.class(),
            ErrorClass::Transient
        );
        assert_eq!(
            PlatformCredentialsError::Refused.class(),
            ErrorClass::Unauthenticated
        );
        let cert = PlatformClientCertificate::new(
            vec![b"leaf-der".to_vec()],
            Zeroizing::new(b"sentinel-key-bytes".to_vec()),
            "0a:0b",
            jiff::Timestamp::UNIX_EPOCH,
        );
        let shown = format!("{cert:?}");
        assert!(shown.contains("0a:0b"), "{shown}");
        assert!(!shown.contains("sentinel"), "{shown}");
        assert!(!shown.contains("leaf-der"), "{shown}");
        assert_eq!(cert.expose_key_pkcs8(), b"sentinel-key-bytes");
        assert_eq!(cert.clone().chain_der(), [b"leaf-der".to_vec()]);
    }
}
