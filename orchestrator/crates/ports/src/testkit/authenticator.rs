//! `Authenticator` conformance cases: what the HTTP edge relies on from any authenticator
//! (ADR 0033), stated once and run against the in-memory one, against the proxy-header one and
//! against the JWT one.
//!
//! An implementation supplies an [`AuthFixture`]: the authenticator, credentials it accepts and
//! credentials it refuses, and, when it can be unavailable, a way to make it so. Each case is
//! `async fn(fixture)` and gives up after 10 seconds. The cases that need an authenticator which
//! can be down are skipped when the fixture says it cannot (the proxy header needs nothing).
//!
//! What is deliberately not asserted: the wording of a detail, and how a credential is checked.
//! The claims of a signed token are the business of [`bearer`](super::bearer).

use std::future::Future;
use std::time::Duration;

use orch_core::{Classify, ErrorClass, UserId};

use crate::{AuthError, Authenticator, Credentials, Principal};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(10);

/// Credentials, owned: what a case holds while it lends [`Credentials`] to the authenticator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Presented {
    /// The bearer token.
    pub bearer: Option<String>,
    /// The identity header.
    pub identity_header: Option<String>,
}

impl Presented {
    /// A bearer token alone.
    pub fn bearer(token: impl Into<String>) -> Self {
        Presented {
            bearer: Some(token.into()),
            identity_header: None,
        }
    }

    /// An identity header alone.
    pub fn header(value: impl Into<String>) -> Self {
        Presented {
            bearer: None,
            identity_header: Some(value.into()),
        }
    }

    /// Lent to the authenticator.
    pub fn credentials(&self) -> Credentials<'_> {
        Credentials {
            bearer: self.bearer.as_deref(),
            identity_header: self.identity_header.as_deref(),
        }
    }

    /// Every text this holds, for a case that looks for a credential in an error.
    fn texts(&self) -> impl Iterator<Item = &str> {
        self.bearer
            .as_deref()
            .into_iter()
            .chain(self.identity_header.as_deref())
    }
}

/// An authenticator under test.
pub trait AuthFixture: Send + Sync + 'static {
    /// The authenticator under test.
    type Auth: Authenticator;

    /// The authenticator.
    fn auth(&self) -> &Self::Auth;

    /// Credentials it accepts, and the user they name.
    fn valid(&self) -> (Presented, UserId);

    /// Credentials it must refuse: presented, and not good. At least one. None of them is empty
    /// of credentials (that is the `Missing` case).
    fn refused(&self) -> Vec<Presented>;

    /// Takes whatever the authenticator checks against down (`true`) or brings it back (`false`),
    /// and returns whether it can be taken down at all. An authenticator that needs nothing
    /// outside itself returns `false`, and the cases about availability are skipped.
    fn set_down(&self, down: bool) -> bool;
}

async fn within<T>(case: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, case)
        .await
        .expect("the case timed out")
}

async fn authenticate<F: AuthFixture>(
    f: &F,
    presented: &Presented,
) -> Result<Principal, AuthError> {
    within(f.auth().authenticate(&presented.credentials())).await
}

/// Credentials the authenticator accepts name the user the fixture says.
pub async fn valid_credentials_name_the_user<F: AuthFixture>(f: F) {
    let (presented, user) = f.valid();
    let principal = authenticate(&f, &presented).await.expect("a principal");
    assert_eq!(principal.user, user);
}

/// A request with no credential is `Missing`, a refusal that the caller can fix by logging in.
pub async fn no_credentials_is_missing<F: AuthFixture>(f: F) {
    let err = authenticate(&f, &Presented::default())
        .await
        .expect_err("nobody is let in without a credential");
    assert!(matches!(err, AuthError::Missing), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Unauthenticated);
    assert!(!err.is_retryable());
}

/// A credential that is presented and not good is `Invalid`: never a principal, and never
/// `Unavailable` (the authenticator does not blame its own state for a bad credential).
pub async fn refused_credentials_are_invalid<F: AuthFixture>(f: F) {
    let refused = f.refused();
    assert!(
        !refused.is_empty(),
        "the fixture must offer something to refuse"
    );
    for presented in refused {
        let err = authenticate(&f, &presented)
            .await
            .expect_err("a refused credential is never a principal");
        assert!(
            matches!(err, AuthError::Invalid { .. }),
            "{presented:?}: {err:?}"
        );
        assert_eq!(err.class(), ErrorClass::Unauthenticated);
    }
}

/// No error, in its text or its debug text, holds a credential.
pub async fn the_credential_is_never_in_an_error<F: AuthFixture>(f: F) {
    let mut errors = Vec::new();
    for presented in f.refused() {
        let err = authenticate(&f, &presented).await.expect_err("refused");
        errors.push((presented, err));
    }
    if f.set_down(true) {
        let (presented, _) = f.valid();
        let err = authenticate(&f, &presented).await.expect_err("down");
        errors.push((presented, err));
        f.set_down(false);
    }
    for (presented, err) in errors {
        let shown = format!("{err} | {err:?} | {}", orch_core::report(&err));
        for text in presented.texts().filter(|t| t.len() > 3) {
            assert!(
                !shown.contains(text),
                "an error shows a credential: {shown}"
            );
        }
    }
}

/// An authenticator that cannot check is `Unavailable`: nobody is let in, it is not a refusal
/// (the class is `Transient`), `ready` says so, and it recovers when what it needs comes back.
pub async fn unavailable_is_closed_and_distinguishable<F: AuthFixture>(f: F) {
    if !f.set_down(true) {
        return;
    }
    let (presented, user) = f.valid();
    let err = authenticate(&f, &presented)
        .await
        .expect_err("nobody is let in while it cannot check");
    assert!(matches!(err, AuthError::Unavailable { .. }), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Transient);
    assert!(err.is_retryable());
    let not_ready = within(f.auth().ready()).await;
    assert!(
        matches!(not_ready, Err(AuthError::Unavailable { .. })),
        "{not_ready:?}"
    );
    f.set_down(false);
    let principal = authenticate(&f, &presented).await.expect("recovered");
    assert_eq!(principal.user, user);
    within(f.auth().ready()).await.expect("ready again");
}

/// An authenticator that has what it needs is ready.
pub async fn ready_when_it_can_authenticate<F: AuthFixture>(f: F) {
    let (presented, _) = f.valid();
    authenticate(&f, &presented).await.expect("a principal");
    within(f.auth().ready()).await.expect("ready");
}
