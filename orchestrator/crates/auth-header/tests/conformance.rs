//! The `Authenticator` testkit against the proxy-header authenticator, and what is its own: the
//! normalisation of the e-mail, the development user, the bearer it never reads.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_auth_header::HeaderAuth;
use orch_core::UserId;
use orch_ports::testkit::authenticator::{AuthFixture, Presented};
use orch_ports::{AuthError, Authenticator, Credentials};

struct Header(HeaderAuth);

impl AuthFixture for Header {
    type Auth = HeaderAuth;

    fn auth(&self) -> &HeaderAuth {
        &self.0
    }

    fn valid(&self) -> (Presented, UserId) {
        (
            Presented::header(" Alice@Example.com "),
            UserId::new("alice@example.com"),
        )
    }

    fn refused(&self) -> Vec<Presented> {
        vec![
            Presented::header(""),
            Presented::header("   "),
            Presented::header("no-at-sign"),
        ]
    }

    fn set_down(&self, _down: bool) -> bool {
        // It needs nothing outside itself.
        false
    }
}

async fn make() -> Option<Header> {
    Some(Header(HeaderAuth::new()))
}

orch_ports::authenticator_conformance!(make);

fn header(value: &str) -> Credentials<'_> {
    Credentials {
        bearer: None,
        identity_header: Some(value),
        ..Credentials::default()
    }
}

#[tokio::test]
async fn the_principal_is_the_normalised_email_with_no_roles() {
    let p = HeaderAuth::new()
        .authenticate(&header(" Bob@Example.COM "))
        .await
        .unwrap();
    assert_eq!(p.user.as_str(), "bob@example.com");
    assert_eq!(p.email.as_deref(), Some("bob@example.com"));
    assert!(p.name.is_none() && p.roles.is_empty());
}

#[tokio::test]
async fn a_development_user_serves_only_a_request_without_the_header() {
    let auth = HeaderAuth::new().with_dev_user(UserId::new("Dev@Example.com"));
    assert_eq!(auth.dev_user().unwrap().as_str(), "dev@example.com");
    let none = auth.authenticate(&Credentials::default()).await.unwrap();
    assert_eq!(none.user.as_str(), "dev@example.com");
    // A header that is there and is not an address is refused, the dev user notwithstanding.
    for garbage in ["", "nobody", "  "] {
        let err = auth.authenticate(&header(garbage)).await.unwrap_err();
        assert!(
            matches!(err, AuthError::Invalid { .. }),
            "{garbage:?}: {err:?}"
        );
    }
    // A real header wins over the dev user.
    let alice = auth
        .authenticate(&header("alice@example.com"))
        .await
        .unwrap();
    assert_eq!(alice.user.as_str(), "alice@example.com");
}

#[tokio::test]
async fn a_bearer_token_is_not_read() {
    let with_bearer = Credentials {
        bearer: Some("eyJ.a.b"),
        identity_header: None,
        ..Credentials::default()
    };
    let err = HeaderAuth::new()
        .authenticate(&with_bearer)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Missing), "{err:?}");
    assert!(!HeaderAuth::new().accepts_bearer());
}
