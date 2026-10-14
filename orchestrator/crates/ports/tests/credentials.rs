//! The credential broker port (ADR 0046, the amendment of 2026-10-11), through the public API: the
//! lease, the OAuth state, the redirect URI and the OAuth start, the classed errors, and that the
//! trait can be implemented as designed. The broker itself is `orch-credential-broker`, which runs the
//! conformance macro; the stub below is the smallest thing that proves the shapes fit and pins the contract the trait
//! documents (a revoked credential, the URL a credential was issued for, a kind with nothing).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::HashMap;
use std::sync::Mutex;

use jiff::Timestamp;
use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::memory::MemoryUserToolServers;
use orch_ports::{
    CredentialBroker, CredentialError, CredentialLease, CredentialRef, DefinitionError,
    EgressPolicy, MAX_OAUTH_STATE_BYTES, MIN_OAUTH_STATE_BYTES, OAuthStart, OAuthState,
    OAuthStateError, RedirectUri, RedirectUriError, ToolSecret, UserAuth, UserToolServer,
    UserToolServerId, UserToolServerState, UserToolServerStore,
};

const SECRET: &str = "tok-9c2f8d-do-not-print";
const STATE: &str = "Zm9vYmFyYmF6cXV4MTIzNDU2";

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(second).unwrap()
}

fn state(text: &str) -> OAuthState {
    OAuthState::new(text).unwrap()
}

// ---------------------------------------------------------------------------------- the lease

#[test]
fn a_bearer_lease_is_the_authorization_header_with_the_bare_token() {
    let lease = CredentialLease::bearer(ToolSecret::new(SECRET), at(100));
    assert_eq!(lease.header_name(), "Authorization");
    assert_eq!(lease.secret().expose(), SECRET);
    assert_eq!(lease.expires_at(), at(100));
    assert!(lease.is_bearer());
    assert!(matches!(
        lease,
        CredentialLease::Bearer { ref token, .. } if token.expose() == SECRET
    ));
}

#[test]
fn a_header_lease_is_the_header_the_person_named() {
    let lease = CredentialLease::header("X-Api-Key", ToolSecret::new(SECRET), at(100)).unwrap();
    assert_eq!(lease.header_name(), "X-Api-Key");
    assert_eq!(lease.secret().expose(), SECRET);
    assert_eq!(lease.expires_at(), at(100));
    assert!(!lease.is_bearer());
    assert!(matches!(
        lease,
        CredentialLease::Header { ref name, ref value, .. }
            if name == "X-Api-Key" && value.expose() == SECRET
    ));
}

#[test]
fn a_header_lease_is_in_a_header_a_person_may_name() {
    // the same names an `api_key` kind refuses: a lease cannot put a credential in `Authorization`
    // (that is the bearer's), `Host`, the framing headers or the client's own
    for owned in [
        "Authorization",
        "authorization",
        "Host",
        "Content-Length",
        "Cookie",
        "Mcp-Session-Id",
        "Proxy-Authorization",
    ] {
        assert_eq!(
            CredentialLease::header(owned, ToolSecret::new("t"), at(1)).unwrap_err(),
            DefinitionError::ReservedHeader,
            "{owned}"
        );
    }
    for bad in ["", "X Api Key", "X-Api-Key\r\nHost: evil"] {
        assert_eq!(
            CredentialLease::header(bad, ToolSecret::new("t"), at(1)).unwrap_err(),
            DefinitionError::BadHeader,
            "{bad:?}"
        );
    }
}

#[test]
fn a_lease_ends_at_its_time() {
    for lease in [
        CredentialLease::bearer(ToolSecret::new(SECRET), at(100)),
        CredentialLease::header("X-Api-Key", ToolSecret::new(SECRET), at(100)).unwrap(),
    ] {
        assert!(!lease.is_expired_at(at(99)));
        assert!(lease.is_expired_at(at(100)));
        assert!(lease.is_expired_at(at(101)));
    }
}

#[test]
fn a_lease_prints_everything_but_its_value() {
    let header = CredentialLease::header("X-Api-Key", ToolSecret::new(SECRET), at(100)).unwrap();
    let bearer = CredentialLease::bearer(ToolSecret::new(SECRET), at(100));
    for lease in [&header, &bearer] {
        for text in [
            format!("{lease:?}"),
            format!("{lease:#?}"),
            format!("{:?}", Some(lease)),
        ] {
            assert!(!text.contains(SECRET), "{text}");
            assert!(text.contains("<redacted>"), "{text}");
        }
    }
    assert!(format!("{header:?}").contains("X-Api-Key"));
}

/// Compiles for a type that does not implement `Trait`, and is ambiguous (does not compile) for one
/// that does: a check that a type has *not* got a trait, without a dependency.
macro_rules! assert_not_impl {
    ($ty:ty, $tr:path) => {{
        trait AmbiguousIfImpl<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
        impl<T: ?Sized + $tr> AmbiguousIfImpl<u8> for T {}
        let _ = <$ty as AmbiguousIfImpl<_>>::check;
    }};
}

#[test]
fn a_lease_cannot_be_compared_or_copied() {
    // `==` on a credential is not a constant-time comparison, and a lease is used once: moved, never
    // cloned. If someone derives one of these, this test stops compiling.
    assert_not_impl!(CredentialLease, PartialEq);
    assert_not_impl!(CredentialLease, Eq);
    assert_not_impl!(CredentialLease, Clone);
}

// ------------------------------------------------------------------------------ the OAuth state

#[test]
fn a_state_is_sixteen_to_a_hundred_and_twenty_eight_unreserved_characters() {
    assert_eq!((MIN_OAUTH_STATE_BYTES, MAX_OAUTH_STATE_BYTES), (16, 128));
    for good in [
        STATE,
        "abcdefghijklmnop",
        "ABCDEFGHIJKLMNOP",
        "0123456789-._~abc",
        &"a".repeat(128),
        "018f3c4e-7b1a-7c2d-9e5f-0123456789ab",
    ] {
        let parsed = OAuthState::new(good).expect(good);
        assert_eq!(parsed.as_str(), good);
    }
    for bad in [
        "",
        "st-1",
        "abcdefghijklmno",
        &"a".repeat(129),
        "abcdefghijklmnop qrs",
        "abcdefghijklmnop&code=x",
        "abcdefghijklmnop%2f",
        "abcdefghijklmnop/..",
        "abcdefghijklmnop+x",
        "abcdefghijklmnop=",
        "abcdefghijklmnop\n",
        "abcdefghijklmnop\u{e9}",
        "abcdefghijklmnop\"",
    ] {
        assert_eq!(
            OAuthState::new(bad).unwrap_err(),
            OAuthStateError,
            "{bad:?}"
        );
    }
    assert_eq!(OAuthStateError.class(), ErrorClass::Invalid);
    assert!(OAuthStateError.to_string().contains("16"));
}

#[test]
fn states_are_compared_by_content_and_print_nothing() {
    assert_eq!(state(STATE), state(STATE));
    assert_ne!(state(STATE), state("Zm9vYmFyYmF6cXV4MTIzNDU3"));
    // a prefix, or a longer text with the same start, is another state
    assert_ne!(state(STATE), state(&STATE[..20]));
    assert_ne!(state(&STATE[..20]), state(STATE));
    let text = format!("{:?} {:#?}", state(STATE), state(STATE));
    assert_eq!(text, "<redacted> <redacted>");
}

// ------------------------------------------------------------------------------- the redirect

#[test]
fn a_redirect_is_https_or_http_on_a_loopback_literal() {
    for (given, normal) in [
        (
            "https://app.example.com/api/user-tool-servers/oauth/callback",
            "https://app.example.com/api/user-tool-servers/oauth/callback",
        ),
        ("https://app.example.com", "https://app.example.com/"),
        (
            "HTTPS://APP.example.com:8443/cb",
            "https://app.example.com:8443/cb",
        ),
        (
            "https://app.example.com/cb?tab=servers",
            "https://app.example.com/cb?tab=servers",
        ),
        (
            "http://127.0.0.1:53682/callback",
            "http://127.0.0.1:53682/callback",
        ),
        ("http://127.0.0.1/callback", "http://127.0.0.1/callback"),
        ("http://[::1]:53682/callback", "http://[::1]:53682/callback"),
        // the same loopback address, written another way: kept in the normal form
        ("http://0x7f.1/cb", "http://127.0.0.1/cb"),
        ("http://[0:0:0:0:0:0:0:1]:80/cb", "http://[::1]/cb"),
    ] {
        let redirect = RedirectUri::parse(given).expect(given);
        assert_eq!(redirect.as_str(), normal);
        assert_eq!(redirect.to_string(), normal);
    }
}

#[test]
fn a_redirect_is_nothing_else() {
    for bad in [
        "",
        "/api/user-tool-servers/oauth/callback",
        "app.example.com/cb",
        // a fragment (RFC 6749, section 3.1.2)
        "https://app.example.com/cb#done",
        "http://127.0.0.1:53682/cb#done",
        // http, but not on a loopback literal: the name can be pointed anywhere, and the rest is
        // the open network
        "http://app.example.com/cb",
        "http://localhost:53682/cb",
        "http://127.0.0.2:53682/cb",
        "http://10.0.0.1/cb",
        "http://[::2]/cb",
        "http://[::ffff:127.0.0.1]/cb",
        // other schemes: a custom scheme is not accepted here, and the others are no redirect
        "ftp://app.example.com/cb",
        "javascript:alert(1)",
        "data:text/html,x",
        "com.example.app:/cb",
        "file:///etc/passwd",
        // user information, port 0, no host
        "https://user:pw@app.example.com/cb",
        "https://user@app.example.com/cb",
        "https://app.example.com:0/cb",
        "https://",
        // too long
        &format!("https://app.example.com/{}", "a".repeat(2048)),
    ] {
        assert_eq!(
            RedirectUri::parse(bad).unwrap_err(),
            RedirectUriError,
            "{bad:?}"
        );
    }
    assert_eq!(RedirectUriError.class(), ErrorClass::Invalid);
    assert!(!RedirectUriError.to_string().is_empty());
}

// ----------------------------------------------------------------------------- the OAuth start

#[test]
fn an_oauth_start_prints_neither_the_url_nor_the_state_but_says_when_it_ends() {
    let start = OAuthStart {
        authorize_url: format!(
            "https://auth.example.com/authorize?state={STATE}&code_challenge=abc"
        ),
        state: state(STATE),
        expires_at: at(600),
    };
    assert_eq!(start.state.as_str(), STATE);
    assert_eq!(start.expires_at, at(600));
    let text = format!("{start:?} {:?}", start.state);
    assert!(
        !text.contains(STATE) && !text.contains("auth.example.com"),
        "{text}"
    );
    assert!(text.contains("expires_at"), "{text}");
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

/// What a stub broker keeps for a server: the token, and the URL it was issued for.
struct Kept {
    url: String,
    token: ToolSecret,
}

/// A broker over a definition store that keeps what it was given in a mutex: just enough to run the
/// five calls the way the trait documents them.
struct Stub {
    servers: MemoryUserToolServers,
    kept: Mutex<HashMap<(UserId, UserToolServerId), Kept>>,
    pending: Mutex<HashMap<(UserId, UserToolServerId), (OAuthState, String)>>,
}

impl Stub {
    fn new(servers: MemoryUserToolServers) -> Self {
        Stub {
            servers,
            kept: Mutex::default(),
            pending: Mutex::default(),
        }
    }

    /// The definition, as the broker reads it: `Unknown` when there is none.
    async fn definition(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<UserToolServer, CredentialError> {
        self.servers
            .get(owner, server)
            .await
            .map_err(|e| CredentialError::unavailable(e.to_string()))?
            .map(orch_ports::StoredServer::into_server)
            .ok_or(CredentialError::Unknown)
    }
}

impl CredentialBroker for Stub {
    async fn store(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        material: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        let definition = self.definition(owner, server).await?;
        if !definition.auth().needs_credential() {
            return Err(CredentialError::Unknown);
        }
        // the credential is kept with the URL it is issued for
        self.kept.lock().unwrap().insert(
            (owner.clone(), server.clone()),
            Kept {
                url: definition.url().to_owned(),
                token: material.clone(),
            },
        );
        Ok(CredentialRef::new("cr-1").unwrap())
    }

    async fn begin_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        redirect: &RedirectUri,
    ) -> Result<OAuthStart, CredentialError> {
        let definition = self.definition(owner, server).await?;
        if !matches!(definition.auth(), UserAuth::OAuth2 { .. }) {
            return Err(CredentialError::Unknown);
        }
        let state = state(STATE);
        self.pending.lock().unwrap().insert(
            (owner.clone(), server.clone()),
            (state.clone(), definition.url().to_owned()),
        );
        Ok(OAuthStart {
            authorize_url: format!(
                "https://auth.example.com/authorize?redirect_uri={redirect}&state={STATE}"
            ),
            state,
            expires_at: at(600),
        })
    }

    async fn complete_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        state: &OAuthState,
        code: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        let key = (owner.clone(), server.clone());
        // used once
        let (expected, url) = self
            .pending
            .lock()
            .unwrap()
            .remove(&key)
            .ok_or(CredentialError::Unknown)?;
        if &expected != state {
            return Err(CredentialError::Unknown);
        }
        self.kept.lock().unwrap().insert(
            key,
            Kept {
                url,
                token: code.clone(),
            },
        );
        Ok(CredentialRef::new("cr-2").unwrap())
    }

    async fn lease(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        let definition = self.definition(owner, server).await?;
        if !definition.auth().needs_credential() {
            return Err(CredentialError::Unknown);
        }
        let kept = self.kept.lock().unwrap();
        // revoked, or never given: the person must authorize again
        let Some(kept) = kept.get(&(owner.clone(), server.clone())) else {
            return Err(CredentialError::NeedsReauth);
        };
        // issued for another URL than the server's now: not lent
        if kept.url != definition.url() {
            return Err(CredentialError::NeedsReauth);
        }
        match definition.auth() {
            UserAuth::ApiKey { header } => {
                CredentialLease::header(header, kept.token.clone(), at(100))
                    .map_err(|_| CredentialError::NeedsReauth)
            }
            _ => Ok(CredentialLease::bearer(kept.token.clone(), at(100))),
        }
    }

    async fn revoke(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        let key = (owner.clone(), server.clone());
        self.kept.lock().unwrap().remove(&key);
        self.pending.lock().unwrap().remove(&key);
        Ok(())
    }
}

fn id(suffix: &str) -> UserToolServerId {
    UserToolServerId::parse(&format!("u-{suffix}")).unwrap()
}

fn definition(suffix: &str, url: &str, auth: UserAuth) -> UserToolServer {
    let target = EgressPolicy::new().check_url(url).unwrap();
    UserToolServer::new(id(suffix), "A server", &target, auth).unwrap()
}

fn owner() -> UserId {
    UserId::new("alice@example.test")
}

/// Written against the port, as the relay and the API are.
async fn connect_and_use<B: CredentialBroker>(
    broker: &B,
    owner: &UserId,
    server: &UserToolServerId,
) -> Result<String, CredentialError> {
    broker
        .store(owner, server, &ToolSecret::new(SECRET))
        .await?;
    let lease = broker.lease(owner, server).await?;
    Ok(lease.secret().expose().to_owned())
}

#[tokio::test]
async fn the_port_can_be_implemented_and_used_as_designed() {
    let servers = MemoryUserToolServers::new();
    let owner = owner();
    let bearer = definition(
        "aaaaaaaaaa",
        "https://mcp.example.com/mcp",
        UserAuth::Bearer,
    );
    let oauth = definition(
        "bbbbbbbbbb",
        "https://mcp.example.com/other",
        UserAuth::oauth2(vec!["repo".into()], None).unwrap(),
    );
    servers.insert(&owner, &bearer).await.unwrap();
    servers.insert(&owner, &oauth).await.unwrap();
    let broker = Stub::new(servers);

    assert_eq!(
        connect_and_use(&broker, &owner, bearer.id()).await.unwrap(),
        SECRET
    );

    // the flow: begin, then complete with the state the broker gave
    let redirect =
        RedirectUri::parse("https://app.example.com/api/user-tool-servers/oauth/callback").unwrap();
    let start = broker
        .begin_oauth(&owner, oauth.id(), &redirect)
        .await
        .unwrap();
    assert!(
        start
            .authorize_url
            .starts_with("https://auth.example.com/authorize")
    );
    assert!(start.expires_at > at(0));
    let reference = broker
        .complete_oauth(&owner, oauth.id(), &start.state, &ToolSecret::new("code-1"))
        .await
        .unwrap();
    assert_eq!(reference.as_str(), "cr-2");
    // a state is used once, and another is not one
    let again = broker
        .complete_oauth(&owner, oauth.id(), &start.state, &ToolSecret::new("code-1"))
        .await;
    assert!(matches!(again, Err(CredentialError::Unknown)));
    broker
        .begin_oauth(&owner, oauth.id(), &redirect)
        .await
        .unwrap();
    let wrong = broker
        .complete_oauth(
            &owner,
            oauth.id(),
            &state("a-state-nobody-gave-us"),
            &ToolSecret::new("code-1"),
        )
        .await;
    assert!(matches!(wrong, Err(CredentialError::Unknown)));
    // begin_oauth is for an oauth2 server only
    assert!(matches!(
        broker.begin_oauth(&owner, bearer.id(), &redirect).await,
        Err(CredentialError::Unknown)
    ));
}

#[tokio::test]
async fn a_revoked_credential_needs_reauth_and_a_missing_server_is_unknown() {
    let servers = MemoryUserToolServers::new();
    let owner = owner();
    let bearer = definition(
        "aaaaaaaaaa",
        "https://mcp.example.com/mcp",
        UserAuth::Bearer,
    );
    servers.insert(&owner, &bearer).await.unwrap();
    let broker = Stub::new(servers);

    // a server of a kind with a credential, and none given yet: the person has to give one
    assert!(matches!(
        broker.lease(&owner, bearer.id()).await,
        Err(CredentialError::NeedsReauth)
    ));

    broker
        .store(&owner, bearer.id(), &ToolSecret::new(SECRET))
        .await
        .unwrap();
    assert!(broker.lease(&owner, bearer.id()).await.is_ok());

    // revoked: the server is still there, so the person is asked to authorize again
    broker.revoke(&owner, bearer.id()).await.unwrap();
    let revoked = broker.lease(&owner, bearer.id()).await.unwrap_err();
    assert!(
        matches!(revoked, CredentialError::NeedsReauth),
        "{revoked:?}"
    );
    assert_eq!(revoked.class(), ErrorClass::Unauthenticated);
    // revoke is idempotent
    broker.revoke(&owner, bearer.id()).await.unwrap();

    // `Unknown` is "no such server": another person's, or one that was never added
    let nobody = UserId::new("bob@example.test");
    for (who, which) in [(&nobody, bearer.id()), (&owner, &id("zzzzzzzzzz"))] {
        let unknown = broker.lease(who, which).await.unwrap_err();
        assert!(matches!(unknown, CredentialError::Unknown), "{unknown:?}");
        assert_eq!(unknown.class(), ErrorClass::NotFound);
    }
}

#[tokio::test]
async fn a_server_with_no_credential_has_nothing_to_lend() {
    let servers = MemoryUserToolServers::new();
    let owner = owner();
    let open = definition("aaaaaaaaaa", "https://open.example.com/mcp", UserAuth::None);
    servers.insert(&owner, &open).await.unwrap();
    let broker = Stub::new(servers);

    // a kind that has no credential: `Unknown`, for every call, and the relay does not ask
    assert!(!open.auth().needs_credential());
    assert!(matches!(
        broker.lease(&owner, open.id()).await,
        Err(CredentialError::Unknown)
    ));
    assert!(matches!(
        broker.store(&owner, open.id(), &ToolSecret::new("x")).await,
        Err(CredentialError::Unknown)
    ));
}

#[tokio::test]
async fn a_credential_is_lent_only_for_the_url_it_was_issued_for() {
    let servers = MemoryUserToolServers::new();
    let owner = owner();
    let before = definition(
        "aaaaaaaaaa",
        "https://mcp.example.com/mcp",
        UserAuth::Bearer,
    )
    .with_state(UserToolServerState::Connected);
    let revision = servers.insert(&owner, &before).await.unwrap();
    let broker = Stub::new(servers.clone());
    broker
        .store(&owner, before.id(), &ToolSecret::new(SECRET))
        .await
        .unwrap();
    assert!(broker.lease(&owner, before.id()).await.is_ok());

    // the definition is edited to another host (the edit did not revoke): the token is not sent there
    let moved = definition(
        "aaaaaaaaaa",
        "https://elsewhere.example.net/mcp",
        UserAuth::Bearer,
    )
    .with_state(UserToolServerState::Connected);
    servers.replace(&owner, &moved, revision).await.unwrap();
    let refused = broker.lease(&owner, moved.id()).await.unwrap_err();
    assert!(
        matches!(refused, CredentialError::NeedsReauth),
        "{refused:?}"
    );

    // a definition made again under the same id, at another URL, is the same
    servers.delete(&owner, moved.id()).await.unwrap();
    servers
        .insert(
            &owner,
            &definition(
                "aaaaaaaaaa",
                "https://third.example.org/mcp",
                UserAuth::Bearer,
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        broker.lease(&owner, &id("aaaaaaaaaa")).await,
        Err(CredentialError::NeedsReauth)
    ));

    // and a key stored for the new URL is lent for it
    broker
        .store(&owner, &id("aaaaaaaaaa"), &ToolSecret::new("new-token"))
        .await
        .unwrap();
    assert_eq!(
        broker
            .lease(&owner, &id("aaaaaaaaaa"))
            .await
            .unwrap()
            .secret()
            .expose(),
        "new-token"
    );
}

#[tokio::test]
async fn an_api_key_is_lent_in_the_header_the_person_named() {
    let servers = MemoryUserToolServers::new();
    let owner = owner();
    let keyed = definition(
        "aaaaaaaaaa",
        "https://mcp.example.com/mcp",
        UserAuth::api_key("X-Api-Key").unwrap(),
    );
    servers.insert(&owner, &keyed).await.unwrap();
    let broker = Stub::new(servers);
    broker
        .store(&owner, keyed.id(), &ToolSecret::new(SECRET))
        .await
        .unwrap();
    let lease = broker.lease(&owner, keyed.id()).await.unwrap();
    assert_eq!(lease.header_name(), "X-Api-Key");
    assert!(!lease.is_bearer());
    assert_eq!(lease.secret().expose(), SECRET);
}
