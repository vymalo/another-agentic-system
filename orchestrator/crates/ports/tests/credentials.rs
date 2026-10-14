//! The credential broker port (ADR 0046, the amendment of 2026-10-11), through the public API: the
//! lease, the OAuth start, the classed errors, and that the trait can be implemented as designed.
//! The broker itself is slice 2 (`orch-credential-broker`); the stub below is the smallest thing
//! that proves the shapes fit.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Mutex;

use jiff::Timestamp;
use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::{
    CredentialBroker, CredentialError, CredentialLease, CredentialRef, OAuthStart, OAuthState,
    ToolSecret, UserAuth, UserToolServerId,
};

const SECRET: &str = "tok-9c2f8d-do-not-print";

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(second).unwrap()
}

// ---------------------------------------------------------------------------------- the lease

#[test]
fn a_bearer_lease_is_the_authorization_header_with_the_bare_token() {
    let lease = CredentialLease::bearer(ToolSecret::new(SECRET), at(100));
    assert_eq!(lease.header, "Authorization");
    assert_eq!(lease.value.expose(), SECRET);
    assert_eq!(lease.expires_at, at(100));
    assert!(lease.is_authorization());
}

#[test]
fn a_header_lease_is_the_header_the_person_named() {
    let lease = CredentialLease::new("X-Api-Key", ToolSecret::new(SECRET), at(100));
    assert_eq!(lease.header, "X-Api-Key");
    assert!(!lease.is_authorization());
    // the header is compared without regard to case
    assert!(CredentialLease::new("authorization", ToolSecret::new("t"), at(1)).is_authorization());
}

#[test]
fn a_lease_ends_at_its_time() {
    let lease = CredentialLease::bearer(ToolSecret::new(SECRET), at(100));
    assert!(!lease.is_expired_at(at(99)));
    assert!(lease.is_expired_at(at(100)));
    assert!(lease.is_expired_at(at(101)));
}

#[test]
fn a_lease_prints_everything_but_its_value() {
    let lease = CredentialLease::new("X-Api-Key", ToolSecret::new(SECRET), at(100));
    for text in [
        format!("{lease:?}"),
        format!("{lease:#?}"),
        format!("{:?}", Some(&lease)),
    ] {
        assert!(!text.contains(SECRET), "{text}");
        assert!(
            text.contains("X-Api-Key") && text.contains("<redacted>"),
            "{text}"
        );
    }
}

// ----------------------------------------------------------------------------- the OAuth start

#[test]
fn an_oauth_start_prints_neither_the_url_nor_the_state() {
    let start = OAuthStart {
        authorize_url: "https://auth.example.com/authorize?state=st-4711&code_challenge=abc".into(),
        state: OAuthState::new("st-4711"),
    };
    assert_eq!(start.state.as_str(), "st-4711");
    let text = format!("{start:?} {:?}", start.state);
    assert!(
        !text.contains("st-4711") && !text.contains("auth.example.com"),
        "{text}"
    );
}

// ------------------------------------------------------------------------------------- errors

#[test]
fn each_broker_error_has_the_class_that_says_what_to_do() {
    let cases: [(CredentialError, ErrorClass); 3] = [
        // the person has to authorize again: credentials missing or refused
        (CredentialError::NeedsReauth, ErrorClass::Unauthenticated),
        (
            CredentialError::unavailable("the secret store"),
            ErrorClass::Transient,
        ),
        (CredentialError::Unknown, ErrorClass::NotFound),
    ];
    for (error, class) in cases {
        assert_eq!(error.class(), class, "{error}");
        assert!(!error.to_string().is_empty());
    }
    assert!(!CredentialError::NeedsReauth.is_retryable());
    assert!(CredentialError::unavailable("x").is_retryable());
}

#[test]
fn an_unavailable_error_keeps_its_cause_and_no_other_variant_does() {
    let error = CredentialError::unavailable("the authorization server did not answer")
        .with_source(std::io::Error::other("timed out"));
    assert!(std::error::Error::source(&error).is_some());
    let unchanged = CredentialError::Unknown.with_source(std::io::Error::other("x"));
    assert!(matches!(unchanged, CredentialError::Unknown));
}

// ------------------------------------------------------------------------ the trait, in use

/// A broker that keeps the last token it was given in a mutex: just enough to run the five calls.
#[derive(Default)]
struct Stub {
    stored: Mutex<Option<(UserId, UserToolServerId, ToolSecret)>>,
}

impl CredentialBroker for Stub {
    async fn store(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        kind: &UserAuth,
        material: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        if !kind.needs_credential() {
            return Err(CredentialError::Unknown);
        }
        *self.stored.lock().unwrap() = Some((owner.clone(), server.clone(), material.clone()));
        Ok(CredentialRef::new("cr-1").unwrap())
    }

    async fn begin_oauth(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        redirect: &str,
    ) -> Result<OAuthStart, CredentialError> {
        Ok(OAuthStart {
            authorize_url: format!("https://auth.example.com/authorize?redirect_uri={redirect}"),
            state: OAuthState::new("st-1"),
        })
    }

    async fn complete_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        state: &OAuthState,
        code: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        if state.as_str() != "st-1" {
            return Err(CredentialError::Unknown);
        }
        *self.stored.lock().unwrap() = Some((owner.clone(), server.clone(), code.clone()));
        Ok(CredentialRef::new("cr-2").unwrap())
    }

    async fn lease(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        match &*self.stored.lock().unwrap() {
            Some((o, s, token)) if o == owner && s == server => {
                Ok(CredentialLease::bearer(token.clone(), at(100)))
            }
            _ => Err(CredentialError::Unknown),
        }
    }

    async fn revoke(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        *self.stored.lock().unwrap() = None;
        Ok(())
    }
}

/// Written against the port, as the relay and the API are.
async fn connect_and_use<B: CredentialBroker>(
    broker: &B,
    owner: &UserId,
    server: &UserToolServerId,
) -> Result<String, CredentialError> {
    broker
        .store(owner, server, &UserAuth::Bearer, &ToolSecret::new(SECRET))
        .await?;
    let lease = broker.lease(owner, server).await?;
    Ok(lease.value.expose().to_owned())
}

#[tokio::test]
async fn the_port_can_be_implemented_and_used_as_designed() {
    let broker = Stub::default();
    let owner = UserId::new("alice@example.test");
    let server = UserToolServerId::parse("u-abcdefghij").unwrap();

    assert_eq!(
        connect_and_use(&broker, &owner, &server).await.unwrap(),
        SECRET
    );

    // the flow: begin, then complete with the state the broker gave
    let start = broker
        .begin_oauth(
            &owner,
            &server,
            "https://app.example.com/api/user-tool-servers/oauth/callback",
        )
        .await
        .unwrap();
    assert!(
        start
            .authorize_url
            .starts_with("https://auth.example.com/authorize")
    );
    let reference = broker
        .complete_oauth(&owner, &server, &start.state, &ToolSecret::new("code-1"))
        .await
        .unwrap();
    assert_eq!(reference.as_str(), "cr-2");
    let wrong = broker
        .complete_oauth(
            &owner,
            &server,
            &OAuthState::new("another"),
            &ToolSecret::new("code-1"),
        )
        .await;
    assert!(matches!(wrong, Err(CredentialError::Unknown)));

    // a kind with nothing to keep has no credential
    let none = broker
        .store(&owner, &server, &UserAuth::None, &ToolSecret::new("x"))
        .await;
    assert!(matches!(none, Err(CredentialError::Unknown)));

    broker.revoke(&owner, &server).await.unwrap();
    assert!(matches!(
        broker.lease(&owner, &server).await,
        Err(CredentialError::Unknown)
    ));
}
