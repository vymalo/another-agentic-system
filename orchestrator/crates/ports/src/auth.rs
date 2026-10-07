//! The authentication port: who is calling, from the credentials the request carried (ADR 0033,
//! ADR 0009).
//!
//! The orchestrator is an OAuth2 resource server: a request may carry a signed bearer token
//! (`Authorization: Bearer <JWT>`), a token bound to the caller's key (`Authorization: DPoP
//! <JWT>` and a `DPoP` proof header, RFC 9449, ADR 0054), and a deployment that still has only a
//! proxy in front may carry the identity the proxy vouches for (`X-Auth-Request-Email`). The HTTP
//! edge reads them out of the request, hands them over as [`Credentials`] and learns a
//! [`Principal`], or why not.
//! It never reads a token or a header itself: which credential counts, and how it is checked, is
//! the implementation's, chosen at build time and by configuration (`auth.mode`).
//!
//! An authenticator **fails closed**. It answers one of
//! - a [`Principal`];
//! - [`AuthError::Missing`]: it was given nothing it reads;
//! - [`AuthError::Invalid`]: it was given something and refused it;
//! - [`AuthError::Unavailable`]: it cannot tell, because what it checks against cannot be read
//!   (the key set of the token issuer is down). That is not a refusal, and the edge says so (503,
//!   not 401), but nobody is let in either.
//!
//! No error carries the credential, and the [`Debug`](std::fmt::Debug) of [`Credentials`] hides it.

use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;

use orch_core::{BoxError, Classify, ErrorClass, Timestamp, UserId};

/// What a request carried that may say who is calling. Each part is `None` when the request had
/// none, and `Some("")` when it had one that is empty or cannot be read as text, so that a
/// garbled credential is refused and never taken for an absent one.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Credentials<'a> {
    /// The token of `Authorization: Bearer <token>`, without the scheme. A request with another
    /// scheme (`Basic`) has none.
    pub bearer: Option<&'a str>,
    /// The value of the identity header a proxy in front sets (`X-Auth-Request-Email`).
    pub identity_header: Option<&'a str>,
    /// What a request with `Authorization: DPoP <token>` carried (RFC 9449). `None` for a request
    /// with another scheme; such a request has no `bearer` either.
    pub dpop: Option<DpopCredentials<'a>>,
}

impl fmt::Debug for Credentials<'_> {
    /// Says which credentials are present, never what they are.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("bearer", &self.bearer.map(|_| "<redacted>"))
            .field(
                "identity_header",
                &self.identity_header.map(|_| "<redacted>"),
            )
            .field("dpop", &self.dpop)
            .finish()
    }
}

/// What a `DPoP` request carried (RFC 9449): the access token, the proofs, and the request they
/// are about. The edge reads the request's method and path as the orchestrator received them: it
/// cannot rebuild the public URL, which the authenticator joins to its configured public origins.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DpopCredentials<'a> {
    /// The token of `Authorization: DPoP <token>`, without the scheme; `""` when there is none
    /// (so that it is refused and never taken for an absent one).
    pub token: &'a str,
    /// The value of every `DPoP` header of the request, in order; one that cannot be read as text
    /// is `""`. A request has exactly one proof: none, or two, is a refusal the authenticator makes.
    pub proofs: &'a [&'a str],
    /// The request's method (`GET`).
    pub method: &'a str,
    /// The request's path, without the query, as it arrived (`/api/threads`).
    pub path: &'a str,
}

impl fmt::Debug for DpopCredentials<'_> {
    /// Says what the request was and how many proofs it had, never the token or a proof.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DpopCredentials")
            .field("token", &"<redacted>")
            .field("proofs", &self.proofs.len())
            .field("method", &self.method)
            .field("path", &self.path)
            .finish()
    }
}

/// A role of a person: a name the identity provider gives (a group, a realm role), which the
/// orchestrator's configuration maps to permissions (ADR 0033, `auth.roles`). Compared exactly.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Role(String);

impl Role {
    /// The role named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Role(name.into())
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who is calling, as an authenticator established it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// The key of everything the person owns (`threads.owner`): the e-mail, trimmed and lower-cased
    /// (`auth.jwt.userClaim` names the claim; ADR 0033).
    pub user: UserId,
    /// The person's e-mail address, when the credential says one. Informative: the owner key is
    /// [`Principal::user`].
    pub email: Option<String>,
    /// The person's display name, when the credential says one.
    pub name: Option<String>,
    /// The roles the credential carries. Empty for a credential that has none (the proxy header).
    pub roles: BTreeSet<Role>,
    /// When the credential stops being valid (a token's `exp`); `None` for one that does not run
    /// out (the proxy header, a static token). A stream opened with it ends no later than this
    /// (plus the leeway), and the client reconnects with a fresh credential (ADR 0033).
    pub expires_at: Option<Timestamp>,
}

impl Principal {
    /// A principal that is only a user: no e-mail beyond the key, no name, no roles, and a
    /// credential that does not run out.
    pub fn of(user: UserId) -> Self {
        Principal {
            user,
            email: None,
            name: None,
            roles: BTreeSet::new(),
            expires_at: None,
        }
    }
}

/// Which credential an [`AuthError::Invalid`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// The bearer token.
    Bearer,
    /// The identity header of a proxy.
    IdentityHeader,
    /// The `DPoP` proof of a request (RFC 9449): `WWW-Authenticate: DPoP error="invalid_dpop_proof"`.
    DpopProof,
    /// A token presented with DPoP, or a bound token presented as `Bearer`, that is refused on its
    /// own or because it is not bound to the proof's key: `DPoP error="invalid_token"`.
    DpopToken,
}

/// Why a request was not authenticated.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthError {
    /// The request carried no credential this authenticator reads.
    #[error("no credentials")]
    Missing,
    /// A credential was presented and refused (a token that is expired, not for this API, not
    /// signed by the issuer, malformed; a header that is not an e-mail address). `detail` is fit
    /// to show the caller and to log: it never holds the credential.
    #[error("invalid {credential:?} credentials: {detail}")]
    Invalid {
        /// The credential that was refused.
        credential: CredentialKind,
        /// Why, in a few words.
        detail: String,
    },
    /// What the credential is checked against cannot be read right now (the issuer's key set is
    /// down and was never fetched, or is too old to trust). Retrying later may help; nobody is
    /// let in meanwhile.
    #[error("authentication is unavailable: {detail}")]
    Unavailable {
        /// What is unavailable, without URLs or secrets.
        detail: String,
        /// The cause, for the operator's log (it may hold a URL).
        #[source]
        source: Option<BoxError>,
    },
    /// This process has no authenticator ([`RefuseAll`]).
    #[error("no authenticator is configured")]
    NotConfigured,
}

impl AuthError {
    /// A bearer token that was refused.
    pub fn invalid_bearer(detail: impl Into<String>) -> Self {
        AuthError::Invalid {
            credential: CredentialKind::Bearer,
            detail: detail.into(),
        }
    }

    /// A DPoP proof that was refused.
    pub fn invalid_dpop_proof(detail: impl Into<String>) -> Self {
        AuthError::Invalid {
            credential: CredentialKind::DpopProof,
            detail: detail.into(),
        }
    }

    /// A token that was refused with the DPoP scheme, or a bound token sent as a bearer.
    pub fn invalid_dpop_token(detail: impl Into<String>) -> Self {
        AuthError::Invalid {
            credential: CredentialKind::DpopToken,
            detail: detail.into(),
        }
    }

    /// An identity header that was refused.
    pub fn invalid_header(detail: impl Into<String>) -> Self {
        AuthError::Invalid {
            credential: CredentialKind::IdentityHeader,
            detail: detail.into(),
        }
    }

    /// What the credential is checked against cannot be read.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        AuthError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause of an `Unavailable` error; the other variants have none and
    /// are returned unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            AuthError::Unavailable { detail, .. } => AuthError::Unavailable {
                detail,
                source: Some(source.into()),
            },
            other @ (AuthError::Missing | AuthError::Invalid { .. } | AuthError::NotConfigured) => {
                other
            }
        }
    }
}

impl Classify for AuthError {
    fn class(&self) -> ErrorClass {
        match self {
            AuthError::Missing | AuthError::Invalid { .. } => ErrorClass::Unauthenticated,
            AuthError::Unavailable { .. } => ErrorClass::Transient,
            AuthError::NotConfigured => ErrorClass::Unsupported,
        }
    }
}

/// Establishes who a request is from.
///
/// An implementation stores nothing the orchestrator reasons on (ADR 0001): a cache of the
/// issuer's keys is a copy it may lose at any moment. It never logs or returns a credential.
pub trait Authenticator: Send + Sync + 'static {
    /// The caller of a request that carried `credentials`.
    ///
    /// # Errors
    /// [`AuthError`]: `Missing` and `Invalid` are refusals; `Unavailable` is not one, and nobody
    /// is let in either.
    fn authenticate(
        &self,
        credentials: &Credentials<'_>,
    ) -> impl Future<Output = Result<Principal, AuthError>> + Send;

    /// Whether this authenticator can authenticate now, for `/readyz`: `Ok` for one that needs
    /// nothing outside itself, `Unavailable` for one whose issuer keys were never fetched. It may
    /// try to fetch them, and it answers within a bounded time.
    ///
    /// # Errors
    /// [`AuthError::Unavailable`] when it cannot.
    fn ready(&self) -> impl Future<Output = Result<(), AuthError>> + Send {
        async { Ok(()) }
    }

    /// Whether a bearer token is a credential it reads, so that a 401 may say
    /// `WWW-Authenticate: Bearer` (RFC 6750 §3).
    fn accepts_bearer(&self) -> bool {
        false
    }

    /// Whether `Authorization: DPoP` is a credential it reads, so that a 401 may also say
    /// `WWW-Authenticate: DPoP algs="..."` (RFC 9449 §7.1). `false` unless it is configured: a
    /// request with the `DPoP` scheme is then refused (fail closed).
    fn accepts_dpop(&self) -> bool {
        false
    }
}

/// The authenticator of a process that authenticates nobody (a worker serves no routes): every
/// request is [`AuthError::NotConfigured`], and it is always ready. Also the default of
/// [`PortSet`](crate::PortSet), so that nothing is let in by a bundle that forgot to choose.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefuseAll;

impl Authenticator for RefuseAll {
    async fn authenticate(&self, _credentials: &Credentials<'_>) -> Result<Principal, AuthError> {
        Err(AuthError::NotConfigured)
    }
}

/// An authenticator made of two, chosen by what the request carries: a request with a bearer token
/// (or a DPoP one) is the `bearer` authenticator's, and **only** its (a token that is refused is
/// never reconsidered as an identity header); a request without one is the `header` authenticator's.
/// This is `auth.mode: jwt_or_proxy_header`, the migration from the proxy header to tokens.
///
/// It holds no adapter type: `B` and `H` are any two [`Authenticator`]s.
#[derive(Debug, Clone)]
pub struct ByCredential<B, H> {
    /// Serves requests that carry a bearer token.
    pub bearer: B,
    /// Serves requests that do not.
    pub header: H,
}

impl<B: Authenticator, H: Authenticator> Authenticator for ByCredential<B, H> {
    async fn authenticate(&self, credentials: &Credentials<'_>) -> Result<Principal, AuthError> {
        if credentials.bearer.is_some() || credentials.dpop.is_some() {
            self.bearer.authenticate(credentials).await
        } else {
            self.header.authenticate(credentials).await
        }
    }

    async fn ready(&self) -> Result<(), AuthError> {
        self.bearer.ready().await?;
        self.header.ready().await
    }

    fn accepts_bearer(&self) -> bool {
        true
    }

    fn accepts_dpop(&self) -> bool {
        self.bearer.accepts_dpop()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_the_class_that_says_what_to_do() {
        let cases: [(AuthError, ErrorClass); 5] = [
            (AuthError::Missing, ErrorClass::Unauthenticated),
            (
                AuthError::invalid_bearer("expired"),
                ErrorClass::Unauthenticated,
            ),
            (
                AuthError::invalid_header("not an e-mail"),
                ErrorClass::Unauthenticated,
            ),
            (AuthError::unavailable("keys"), ErrorClass::Transient),
            (AuthError::NotConfigured, ErrorClass::Unsupported),
        ];
        for (err, class) in cases {
            assert_eq!(err.class(), class, "{err}");
        }
        assert!(AuthError::unavailable("keys").is_retryable());
        assert!(!AuthError::Missing.is_retryable());
    }

    #[test]
    fn credentials_never_print() {
        let c = Credentials {
            bearer: Some("eyJ.secret.token"),
            identity_header: Some("alice@example.com"),
            ..Credentials::default()
        };
        let shown = format!("{c:?}");
        assert!(
            !shown.contains("secret") && !shown.contains("alice"),
            "{shown}"
        );
        assert!(shown.contains("redacted"));
        assert_eq!(
            format!("{:?}", Credentials::default()),
            "Credentials { bearer: None, identity_header: None, dpop: None }"
        );
    }

    #[tokio::test]
    async fn refuse_all_refuses_everyone_and_is_ready() {
        let c = Credentials {
            bearer: Some("t"),
            identity_header: Some("a@b"),
            ..Credentials::default()
        };
        assert!(matches!(
            RefuseAll.authenticate(&c).await,
            Err(AuthError::NotConfigured)
        ));
        assert!(RefuseAll.ready().await.is_ok());
        assert!(!RefuseAll.accepts_bearer());
    }

    /// Accepts exactly one value of the one credential it reads.
    struct Only(&'static str, bool);

    impl Authenticator for Only {
        async fn authenticate(&self, c: &Credentials<'_>) -> Result<Principal, AuthError> {
            let presented = if self.1 { c.bearer } else { c.identity_header };
            match presented {
                None => Err(AuthError::Missing),
                Some(v) if v == self.0 => Ok(Principal::of(UserId::new(v))),
                Some(_) => Err(AuthError::invalid_bearer("no")),
            }
        }
    }

    #[tokio::test]
    async fn a_bearer_decides_alone_and_its_refusal_is_final() {
        let both = ByCredential {
            bearer: Only("token", true),
            header: Only("alice@example.com", false),
        };
        let with = |bearer, header| Credentials {
            bearer,
            identity_header: header,
            ..Credentials::default()
        };
        // A bearer is the bearer authenticator's; the header beside it counts for nothing.
        assert_eq!(
            both.authenticate(&with(Some("token"), None))
                .await
                .unwrap()
                .user
                .as_str(),
            "token"
        );
        assert!(matches!(
            both.authenticate(&with(Some("forged"), Some("alice@example.com")))
                .await,
            Err(AuthError::Invalid { .. })
        ));
        // No bearer: the header authenticator.
        assert_eq!(
            both.authenticate(&with(None, Some("alice@example.com")))
                .await
                .unwrap()
                .user
                .as_str(),
            "alice@example.com"
        );
        assert!(matches!(
            both.authenticate(&with(None, None)).await,
            Err(AuthError::Missing)
        ));
        assert!(both.accepts_bearer());
    }

    #[test]
    fn dpop_credentials_never_print_the_token_or_a_proof() {
        let proofs = ["eyJ.proof-secret.sig"];
        let c = Credentials {
            dpop: Some(DpopCredentials {
                token: "eyJ.token-secret.sig",
                proofs: &proofs,
                method: "GET",
                path: "/api/threads",
            }),
            ..Credentials::default()
        };
        let shown = format!("{c:?}");
        assert!(
            !shown.contains("secret") && shown.contains("redacted"),
            "{shown}"
        );
        assert!(
            shown.contains("/api/threads") && shown.contains("GET"),
            "{shown}"
        );
    }

    #[test]
    fn the_dpop_refusals_have_the_class_of_every_refusal() {
        for err in [
            AuthError::invalid_dpop_proof("bad proof"),
            AuthError::invalid_dpop_token("not bound"),
        ] {
            assert_eq!(err.class(), ErrorClass::Unauthenticated, "{err}");
        }
    }

    #[tokio::test]
    async fn a_dpop_request_is_the_token_authenticators_and_its_refusal_is_final() {
        let both = ByCredential {
            bearer: Only("token", true),
            header: Only("alice@example.com", false),
        };
        let proofs: [&str; 0] = [];
        let c = Credentials {
            identity_header: Some("alice@example.com"),
            dpop: Some(DpopCredentials {
                token: "token",
                proofs: &proofs,
                method: "GET",
                path: "/",
            }),
            ..Credentials::default()
        };
        // `Only` reads `bearer`, which a DPoP request does not have: the token authenticator
        // answered (Missing), and the identity header beside it counted for nothing.
        assert!(matches!(
            both.authenticate(&c).await,
            Err(AuthError::Missing)
        ));
        assert!(!both.accepts_dpop(), "`Only` does not read DPoP");
    }
}
