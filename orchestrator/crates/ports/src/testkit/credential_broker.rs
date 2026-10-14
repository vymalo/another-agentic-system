//! `CredentialBroker` conformance cases: what the API and the relay rely on from any broker of a
//! person's MCP-server credentials (ADR 0046, the amendment of 2026-10-11), stated once and run
//! against the broker of `orch-credential-broker` over an in-memory secret store, an in-memory
//! store of definitions and an authorization server of the test's own.
//!
//! An implementation supplies a [`BrokerFixture`]: a broker, the two stores it is composed over, a
//! clock it reads and the authorization server it talks to. Each case is `async fn(fixture)`, uses
//! owners of its own and gives up after 60 seconds.
//!
//! # The authorization server a fixture stands for
//!
//! It publishes, at the well-known paths of RFC 9728 and RFC 8414, the metadata of the servers
//! [`server_url`](BrokerFixture::server_url) and [`other_server_url`](BrokerFixture::other_server_url)
//! (both protected by it) and of itself, with a registration endpoint (RFC 7591) and a revocation
//! endpoint (RFC 7009), and it **enforces what the broker owes it**, so that a case that gets a
//! token has proved the request was right:
//!
//! - the authorization request has `response_type=code`, a `code_challenge` with
//!   `code_challenge_method=S256`, a `state`, the `redirect_uri` the case gave, a `client_id` it
//!   knows, and a `resource` that is the protected server's URL (RFC 8707);
//! - the token request has the `code_verifier` whose S256 challenge it was given, the same
//!   `redirect_uri` and `resource`, and the client's authentication;
//! - a refresh request has a refresh token it issued and the `resource`;
//! - tokens are bearer tokens that live [`access_ttl`](BrokerFixture::access_ttl), and a refresh
//!   token is not invalidated when a new one is issued (no reuse detection: that is the "kept
//!   open" point 3 of the ADR).
//!
//! What is deliberately not asserted: the wording of any error, the wire format between the broker
//! and the authorization server beyond the list above, how the broker keeps its material, and
//! which of two replicas wins a race.

use std::future::Future;
use std::time::Duration;

use futures::future::{join, join_all};
use jiff::Timestamp;
use orch_core::{Classify, ErrorClass, UserId};
use uuid::Uuid;

use crate::{
    CredentialBroker, CredentialError, CredentialLease, EgressPolicy, OAuthState, RedirectUri,
    SecretServer, SecretStore, ServerRevision, ToolSecret, UserAuth, UserToolServer,
    UserToolServerId, UserToolServerStore,
};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// What the redirect of an authorization brought back to the person's browser.
pub struct Consent {
    /// The authorization code.
    pub code: ToolSecret,
    /// The `state` of the redirect.
    pub state: String,
}

/// What the fixture's authorization server answers to a refresh request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// A new access token.
    Succeeds,
    /// `400` with `{"error": "invalid_grant"}`: the grant is gone.
    InvalidGrant,
    /// `500`: the authorization server is failing.
    ServerError,
}

/// How many requests of each kind the fixture's authorization server has seen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AsRequests {
    /// Token requests with `grant_type=authorization_code`.
    pub code_exchanges: usize,
    /// Token requests with `grant_type=refresh_token`.
    pub refreshes: usize,
    /// Requests to the revocation endpoint.
    pub revocations: usize,
}

/// A broker under test, with the stores it is composed over and an authorization server.
pub trait BrokerFixture: Send + Sync + 'static {
    /// The broker under test.
    type Broker: CredentialBroker;
    /// The store of definitions it reads.
    type Servers: UserToolServerStore;
    /// The store it keeps material in.
    type Secrets: SecretStore;

    /// The broker.
    fn broker(&self) -> &Self::Broker;

    /// A second broker, as a second replica of the orchestrator would be: composed over the **same**
    /// secret store, definitions and authorization server, with the same clock, and nothing else
    /// shared (no lock, no cache).
    fn replica(&self) -> Self::Broker;

    /// The store of definitions the brokers read. A case adds, edits and deletes definitions
    /// through it, as the API would.
    fn servers(&self) -> &Self::Servers;

    /// The secret store the brokers keep material in, for the cases that look at what is kept.
    fn secrets(&self) -> &Self::Secrets;

    /// The egress policy the brokers have: a case builds the URL of a definition with it, as the
    /// API would.
    fn egress(&self) -> EgressPolicy;

    /// The time the brokers read.
    fn now(&self) -> Timestamp;

    /// Moves the time the brokers read forward.
    fn advance(&self, by: Duration);

    /// The URL of an MCP server protected by the authorization server.
    fn server_url(&self) -> String;

    /// The URL of another MCP server protected by the same authorization server.
    fn other_server_url(&self) -> String;

    /// A redirect URI the authorization server accepts.
    fn redirect_uri(&self) -> RedirectUri;

    /// The person's browser: opens `authorize_url` at the authorization server, consents, and
    /// returns what the redirect brought. `Err` says why the authorization server refused the
    /// request (a missing `resource`, a missing challenge, ...).
    fn consent(&self, authorize_url: &str) -> impl Future<Output = Result<Consent, String>> + Send;

    /// How long an access token lives, as the authorization server says (`expires_in`).
    fn access_ttl(&self) -> Duration;

    /// Whether the authorization server's metadata lists `S256` in `code_challenge_methods_supported`
    /// (when it does not, the list is absent).
    fn set_pkce_s256(&self, offered: bool);

    /// What the authorization server answers to refresh requests from now on.
    fn set_refresh_outcome(&self, outcome: RefreshOutcome);

    /// How long the token endpoint holds every request before it answers.
    fn set_token_delay(&self, delay: Duration);

    /// The requests the authorization server has seen so far.
    fn requests(&self) -> AsRequests;

    /// From now on every token, code and refresh token the authorization server issues starts with
    /// `sentinel`, and every error body it sends repeats the sentinel and echoes the code, the
    /// verifier, the refresh token and the client secret it was sent.
    fn plant(&self, sentinel: &str);

    /// Every secret value the authorization server has issued or been sent so far: codes,
    /// verifiers, access and refresh tokens, client secrets.
    fn secrets_seen(&self) -> Vec<String>;
}

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A person no other case uses.
fn owner() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

/// A server at `url`: the definition is made as the API makes it (the guard's normal form of the
/// URL), added to the fixture's store, and returned with its revision.
async fn add_at<F: BrokerFixture>(
    f: &F,
    owner: &UserId,
    auth: UserAuth,
    url: &str,
) -> (UserToolServer, ServerRevision) {
    let target = f
        .egress()
        .check_url(url)
        .expect("a URL the fixture's policy accepts");
    let definition = UserToolServer::new(UserToolServerId::generate(), "A server", &target, auth)
        .expect("a valid definition");
    let revision = f
        .servers()
        .insert(owner, &definition)
        .await
        .expect("insert the definition");
    (definition, revision)
}

async fn add<F: BrokerFixture>(f: &F, owner: &UserId, auth: UserAuth) -> UserToolServer {
    add_at(f, owner, auth, &f.server_url()).await.0
}

fn oauth() -> UserAuth {
    UserAuth::oauth2(Vec::new(), None).expect("an oauth2 kind")
}

/// Runs a whole authorization of `server`: begin, consent, complete.
async fn authorize<F: BrokerFixture>(f: &F, owner: &UserId, server: &UserToolServerId) {
    let start = f
        .broker()
        .begin_oauth(owner, server, &f.redirect_uri())
        .await
        .expect("begin_oauth");
    let consent = f
        .consent(&start.authorize_url)
        .await
        .expect("the authorization server accepted the authorization request");
    f.broker()
        .complete_oauth(owner, server, &start.state, &consent.code)
        .await
        .expect("complete_oauth");
}

/// The seconds from `now` to `at`, rounded up, as a duration (zero when `at` has passed).
fn until<F: BrokerFixture>(f: &F, at: Timestamp) -> Duration {
    let seconds = at.as_second() - f.now().as_second();
    Duration::from_secs(u64::try_from(seconds).unwrap_or(0) + 1)
}

fn bearer_of(lease: &CredentialLease) -> String {
    assert!(lease.is_bearer(), "an OAuth credential is lent as a bearer");
    lease.secret().expose().to_owned()
}

/// What an error says to anyone who prints it: `Display` and `Debug` of the error and of every
/// error under it.
fn everything_said_by(error: &CredentialError) -> String {
    let mut said = format!("{error} {error:?}");
    let mut source = std::error::Error::source(error);
    while let Some(e) = source {
        said.push_str(&format!(" {e} {e:?}"));
        source = e.source();
    }
    said
}

// ----------------------------------------------------------------------- bearer and api_key

/// A bearer token is kept, and lent as a bearer lease that carries the token bare, for a short time.
pub async fn a_bearer_token_is_stored_then_leased<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, UserAuth::Bearer).await;
        let kept = f
            .broker()
            .store(&owner, server.id(), &ToolSecret::new("bearer-token-0001"))
            .await
            .expect("store");
        assert!(!kept.as_str().is_empty());

        let lease = f.broker().lease(&owner, server.id()).await.expect("lease");
        assert!(lease.is_bearer());
        assert_eq!(lease.header_name(), "Authorization");
        assert_eq!(lease.secret().expose(), "bearer-token-0001");
        assert!(lease.expires_at() > f.now(), "a lease that has not ended");
        assert!(
            !lease.is_expired_at(f.now()),
            "a fresh lease is not expired"
        );
        assert!(
            lease.expires_at().as_second() - f.now().as_second() <= 3600,
            "a lease is short"
        );
    })
    .await;
}

/// An API key is kept, and lent in the header the definition names.
pub async fn an_api_key_is_stored_then_leased_in_its_header<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(
            &f,
            &owner,
            UserAuth::api_key("X-Api-Key").expect("a header"),
        )
        .await;
        f.broker()
            .store(&owner, server.id(), &ToolSecret::new("key-0001"))
            .await
            .expect("store");
        let lease = f.broker().lease(&owner, server.id()).await.expect("lease");
        assert!(!lease.is_bearer());
        assert_eq!(lease.header_name(), "X-Api-Key");
        assert_eq!(lease.secret().expose(), "key-0001");
        assert!(lease.expires_at() > f.now());
    })
    .await;
}

/// `store` again replaces what was kept: the next lease is the new credential.
pub async fn storing_again_replaces_the_credential<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, UserAuth::Bearer).await;
        let first = f
            .broker()
            .store(&owner, server.id(), &ToolSecret::new("one"))
            .await
            .expect("first store");
        let second = f
            .broker()
            .store(&owner, server.id(), &ToolSecret::new("two"))
            .await
            .expect("second store");
        assert_ne!(first, second, "a new credential has a new reference");
        let lease = f.broker().lease(&owner, server.id()).await.expect("lease");
        assert_eq!(lease.secret().expose(), "two");
    })
    .await;
}

// ------------------------------------------------------------------------- revoke and unknown

/// After `revoke` a lease is `NeedsReauth` (the server exists, its credential does not), a second
/// `revoke` succeeds, and a server that never had a credential is `NeedsReauth` too.
pub async fn a_revoked_or_never_given_credential_needs_reauth<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, UserAuth::Bearer).await;

        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("nothing was given");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
        assert_eq!(error.class(), ErrorClass::Unauthenticated);

        f.broker()
            .store(&owner, server.id(), &ToolSecret::new("token"))
            .await
            .expect("store");
        f.broker().lease(&owner, server.id()).await.expect("lease");

        f.broker()
            .revoke(&owner, server.id())
            .await
            .expect("revoke");
        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("revoked");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");

        f.broker()
            .revoke(&owner, server.id())
            .await
            .expect("revoke is idempotent");
        assert!(
            f.secrets()
                .get(&owner, &SecretServer::credential(server.id()))
                .await
                .expect("get")
                .is_none(),
            "revoke forgets the credential"
        );
    })
    .await;
}

/// A server that is not there is `Unknown` for every call but `revoke`, which succeeds.
pub async fn a_missing_server_is_unknown<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let id = UserToolServerId::generate();
        let b = f.broker();
        let error = b
            .store(&owner, &id, &ToolSecret::new("x"))
            .await
            .expect_err("store");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        assert_eq!(error.class(), ErrorClass::NotFound);
        let error = b
            .begin_oauth(&owner, &id, &f.redirect_uri())
            .await
            .expect_err("begin_oauth");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        let state = OAuthState::new("AAAAAAAAAAAAAAAAAAAAAAAA").expect("a state");
        let error = b
            .complete_oauth(&owner, &id, &state, &ToolSecret::new("code"))
            .await
            .expect_err("complete_oauth");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        let error = b.lease(&owner, &id).await.expect_err("lease");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        b.revoke(&owner, &id).await.expect("revoke of nothing");
    })
    .await;
}

/// A server whose kind has no credential is `Unknown` for `store`, `lease` and `begin_oauth`, and
/// a server of another kind is `Unknown` for `begin_oauth`.
pub async fn a_server_of_kind_none_or_not_oauth_is_unknown<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let none = add(&f, &owner, UserAuth::None).await;
        let b = f.broker();
        let error = b
            .store(&owner, none.id(), &ToolSecret::new("x"))
            .await
            .expect_err("store of a none server");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        let error = b.lease(&owner, none.id()).await.expect_err("lease");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        let error = b
            .begin_oauth(&owner, none.id(), &f.redirect_uri())
            .await
            .expect_err("begin_oauth of a none server");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        b.revoke(&owner, none.id()).await.expect("revoke");

        let bearer = add(&f, &owner, UserAuth::Bearer).await;
        let error = b
            .begin_oauth(&owner, bearer.id(), &f.redirect_uri())
            .await
            .expect_err("begin_oauth of a bearer server");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
    })
    .await;
}

/// Everything is the owner's alone: another person's server is `Unknown`, and what the first
/// person gave is untouched.
pub async fn another_owners_server_is_unknown<F: BrokerFixture>(f: F) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let server = add(&f, &alice, UserAuth::Bearer).await;
        f.broker()
            .store(&alice, server.id(), &ToolSecret::new("alice-token"))
            .await
            .expect("store");

        let b = f.broker();
        let error = b
            .lease(&bob, server.id())
            .await
            .expect_err("bob's lease of alice's server");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        let error = b
            .store(&bob, server.id(), &ToolSecret::new("bob-token"))
            .await
            .expect_err("bob's store");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        b.revoke(&bob, server.id())
            .await
            .expect("bob's revoke is nothing");

        let lease = b.lease(&alice, server.id()).await.expect("alice's lease");
        assert_eq!(lease.secret().expose(), "alice-token");
    })
    .await;
}

// ----------------------------------------------------------------------------- URL binding

/// A credential stored for one URL is not lent after the definition's URL changes: `NeedsReauth`
/// (departure 15 of slice 1). Giving a credential again for the new URL lends it.
pub async fn a_credential_is_not_lent_for_another_url<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let (server, revision) = add_at(&f, &owner, UserAuth::Bearer, &f.server_url()).await;
        f.broker()
            .store(&owner, server.id(), &ToolSecret::new("for-the-first-url"))
            .await
            .expect("store");
        f.broker().lease(&owner, server.id()).await.expect("lease");

        // the definition is edited to another URL, as `PATCH` would, and the credential is left
        // behind (the API revokes first; this holds when it does not)
        let target = f
            .egress()
            .check_url(&f.other_server_url())
            .expect("the other URL");
        let edited = UserToolServer::new(server.id().clone(), "Edited", &target, UserAuth::Bearer)
            .expect("an edited definition");
        f.servers()
            .replace(&owner, &edited, revision)
            .await
            .expect("replace");

        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("the credential was issued for another URL");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");

        f.broker()
            .store(&owner, server.id(), &ToolSecret::new("for-the-new-url"))
            .await
            .expect("store for the new URL");
        let lease = f.broker().lease(&owner, server.id()).await.expect("lease");
        assert_eq!(lease.secret().expose(), "for-the-new-url");
    })
    .await;
}

/// The same for a definition that was deleted and made again under the same id at another URL.
pub async fn a_credential_is_not_lent_to_a_definition_made_again<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, UserAuth::Bearer).await;
        f.broker()
            .store(&owner, server.id(), &ToolSecret::new("old-grant"))
            .await
            .expect("store");

        f.servers()
            .delete(&owner, server.id())
            .await
            .expect("delete the definition");
        let target = f
            .egress()
            .check_url(&f.other_server_url())
            .expect("the other URL");
        let again = UserToolServer::new(server.id().clone(), "Again", &target, UserAuth::Bearer)
            .expect("a definition");
        f.servers().insert(&owner, &again).await.expect("insert");

        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("the grant belongs to the old definition");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
    })
    .await;
}

/// OAuth tokens are bound too: after the URL changes the access token is not lent, though it is
/// still valid.
pub async fn oauth_tokens_are_not_lent_for_another_url<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let (server, revision) = add_at(&f, &owner, oauth(), &f.server_url()).await;
        authorize(&f, &owner, server.id()).await;
        f.broker().lease(&owner, server.id()).await.expect("lease");

        let target = f
            .egress()
            .check_url(&f.other_server_url())
            .expect("the other URL");
        let edited =
            UserToolServer::new(server.id().clone(), "Edited", &target, oauth()).expect("edit");
        f.servers()
            .replace(&owner, &edited, revision)
            .await
            .expect("replace");

        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("tokens issued for another resource");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
        assert_eq!(
            f.requests().refreshes,
            0,
            "a credential that is not lent is not refreshed either"
        );
    })
    .await;
}

// ------------------------------------------------------------------------------------- OAuth

/// An authorization from begin to lease: the authorization server enforced PKCE `S256`, the
/// `resource` and the redirect URI, the lease is a bearer token, and a second lease asks nobody.
pub async fn the_oauth_happy_path<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        let before = f.now();

        let start = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("begin_oauth");
        assert!(start.expires_at > before, "the flow ends later");
        assert!(
            f.secrets()
                .get(&owner, &SecretServer::pending_flow(server.id()))
                .await
                .expect("get")
                .is_some(),
            "the pending flow is kept"
        );

        let consent = f
            .consent(&start.authorize_url)
            .await
            .expect("the authorization request had PKCE S256, the resource and the redirect URI");
        assert_eq!(
            OAuthState::new(&consent.state).expect("a state"),
            start.state,
            "the redirect brings the state back"
        );

        let kept = f
            .broker()
            .complete_oauth(&owner, server.id(), &start.state, &consent.code)
            .await
            .expect("complete_oauth");
        assert!(!kept.as_str().is_empty());
        assert_eq!(f.requests().code_exchanges, 1);
        assert!(
            f.secrets()
                .get(&owner, &SecretServer::pending_flow(server.id()))
                .await
                .expect("get")
                .is_none(),
            "the flow is forgotten once it is used"
        );

        let lease = f.broker().lease(&owner, server.id()).await.expect("lease");
        assert!(!bearer_of(&lease).is_empty());
        assert!(lease.expires_at() > f.now());

        let again = f.broker().lease(&owner, server.id()).await.expect("lease");
        assert_eq!(bearer_of(&again), bearer_of(&lease));
        assert_eq!(f.requests().refreshes, 0, "a fresh token asks nobody");
    })
    .await;
}

/// An access token that has run out is refreshed, with the refresh token, and the new one is lent.
pub async fn an_expired_access_token_is_refreshed<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        authorize(&f, &owner, server.id()).await;
        let first = bearer_of(&f.broker().lease(&owner, server.id()).await.expect("lease"));

        f.advance(f.access_ttl() + Duration::from_secs(1));
        let second = f.broker().lease(&owner, server.id()).await.expect("lease");
        assert_ne!(bearer_of(&second), first, "a new token");
        assert!(second.expires_at() > f.now());
        assert_eq!(f.requests().refreshes, 1);

        // and the new token is kept: the next lease asks nobody
        let third = f.broker().lease(&owner, server.id()).await.expect("lease");
        assert_eq!(bearer_of(&third), bearer_of(&second));
        assert_eq!(f.requests().refreshes, 1);
    })
    .await;
}

/// A token with less than 30 seconds to live is refreshed; one with more is not.
pub async fn a_token_is_refreshed_thirty_seconds_before_it_runs_out<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        authorize(&f, &owner, server.id()).await;

        // 60 seconds left: lent as it is
        f.advance(f.access_ttl() - Duration::from_secs(60));
        f.broker().lease(&owner, server.id()).await.expect("lease");
        assert_eq!(f.requests().refreshes, 0, "60 seconds are enough");

        // 20 seconds left: refreshed
        f.advance(Duration::from_secs(40));
        f.broker().lease(&owner, server.id()).await.expect("lease");
        assert_eq!(f.requests().refreshes, 1, "20 seconds are not enough");
    })
    .await;
}

/// Leases that need the same refresh at the same time make one request to the authorization
/// server (single flight), and all get the same token.
pub async fn concurrent_leases_refresh_once<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        authorize(&f, &owner, server.id()).await;
        f.advance(f.access_ttl() + Duration::from_secs(1));
        f.set_token_delay(Duration::from_millis(150));

        let leases = join_all((0..8).map(|_| f.broker().lease(&owner, server.id()))).await;
        let tokens: Vec<String> = leases
            .into_iter()
            .map(|lease| bearer_of(&lease.expect("lease")))
            .collect();
        assert!(
            tokens.iter().all(|t| *t == tokens[0]),
            "every caller got the one refreshed token"
        );
        assert_eq!(f.requests().refreshes, 1, "one refresh for eight leases");
    })
    .await;
}

/// Two replicas that refresh at the same time each ask once at most; the one that loses the
/// `rotate` reads what the winner stored and lends that, and does not refresh again.
pub async fn a_refresh_racing_a_replica_is_settled_by_rotate<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        authorize(&f, &owner, server.id()).await;
        f.advance(f.access_ttl() + Duration::from_secs(1));
        f.set_token_delay(Duration::from_millis(300));
        let replica = f.replica();

        let (a, b) = join(
            f.broker().lease(&owner, server.id()),
            replica.lease(&owner, server.id()),
        )
        .await;
        let (a, b) = (a.expect("first replica"), b.expect("second replica"));
        assert_eq!(
            bearer_of(&a),
            bearer_of(&b),
            "the loser lends what the winner stored"
        );
        let asked = f.requests().refreshes;
        assert!(
            (1..=2).contains(&asked),
            "each replica asks once at most, asked {asked}"
        );

        let before = f.requests().refreshes;
        let third = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect("a later lease");
        replica
            .lease(&owner, server.id())
            .await
            .expect("and the replica's");
        assert_eq!(bearer_of(&third), bearer_of(&a));
        assert_eq!(
            f.requests().refreshes,
            before,
            "never refreshed twice in a row"
        );
    })
    .await;
}

/// A refresh that the authorization server refuses with `invalid_grant` means the grant is gone:
/// `NeedsReauth`, and no further request to the authorization server for the next lease.
pub async fn invalid_grant_on_refresh_needs_reauth<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        authorize(&f, &owner, server.id()).await;
        f.advance(f.access_ttl() + Duration::from_secs(1));
        f.set_refresh_outcome(RefreshOutcome::InvalidGrant);

        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("the grant is gone");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
        assert_eq!(f.requests().refreshes, 1);

        // the grant does not come back by itself, and nobody asks again
        f.set_refresh_outcome(RefreshOutcome::Succeeds);
        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("still gone");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
        assert_eq!(f.requests().refreshes, 1, "a dead grant is not asked again");

        // authorizing again makes a new one
        authorize(&f, &owner, server.id()).await;
        f.broker().lease(&owner, server.id()).await.expect("lease");
    })
    .await;
}

/// A refresh that fails because the authorization server is failing is `Unavailable` (transient),
/// and does not cost the person the grant: when the server recovers the next lease refreshes.
pub async fn a_failing_authorization_server_is_unavailable_and_keeps_the_grant<F: BrokerFixture>(
    f: F,
) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        authorize(&f, &owner, server.id()).await;
        f.advance(f.access_ttl() + Duration::from_secs(1));
        f.set_refresh_outcome(RefreshOutcome::ServerError);

        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("the authorization server failed");
        assert!(
            matches!(error, CredentialError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);

        f.set_refresh_outcome(RefreshOutcome::Succeeds);
        let lease = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect("the grant survived");
        assert!(lease.expires_at() > f.now());
    })
    .await;
}

/// A `state` that is not the pending flow's is `Unknown` and does not cost the person the flow.
pub async fn a_wrong_state_is_unknown_and_leaves_the_flow<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        let start = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("begin_oauth");
        let consent = f.consent(&start.authorize_url).await.expect("consent");

        let wrong = OAuthState::new("wrong-state-0123456789").expect("a state");
        let error = f
            .broker()
            .complete_oauth(&owner, server.id(), &wrong, &consent.code)
            .await
            .expect_err("a state nobody began");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        assert_eq!(f.requests().code_exchanges, 0, "the code was not spent");

        f.broker()
            .complete_oauth(&owner, server.id(), &start.state, &consent.code)
            .await
            .expect("the right state still completes the flow");
    })
    .await;
}

/// A `state` is used once: the second `complete_oauth` with it is `Unknown`.
pub async fn a_state_is_used_once<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        let start = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("begin_oauth");
        let consent = f.consent(&start.authorize_url).await.expect("consent");
        f.broker()
            .complete_oauth(&owner, server.id(), &start.state, &consent.code)
            .await
            .expect("first");
        let error = f
            .broker()
            .complete_oauth(&owner, server.id(), &start.state, &consent.code)
            .await
            .expect_err("second");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        assert_eq!(f.requests().code_exchanges, 1, "the code is spent once");
    })
    .await;
}

/// Two callbacks with the same `state` at the same time spend the code once.
pub async fn concurrent_callbacks_spend_the_state_once<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        let start = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("begin_oauth");
        let consent = f.consent(&start.authorize_url).await.expect("consent");
        f.set_token_delay(Duration::from_millis(100));
        let replica = f.replica();

        let (a, b) = join(
            f.broker()
                .complete_oauth(&owner, server.id(), &start.state, &consent.code),
            replica.complete_oauth(&owner, server.id(), &start.state, &consent.code),
        )
        .await;
        let wins = [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count();
        assert_eq!(wins, 1, "one callback wins");
        for result in [a, b] {
            if let Err(error) = result {
                assert!(matches!(error, CredentialError::Unknown), "{error:?}");
            }
        }
        assert_eq!(f.requests().code_exchanges, 1, "the code is spent once");
    })
    .await;
}

/// A flow that has ended is `Unknown`, and the person can begin again.
pub async fn an_expired_flow_is_unknown<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        let start = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("begin_oauth");
        let consent = f.consent(&start.authorize_url).await.expect("consent");

        f.advance(until(&f, start.expires_at));
        let error = f
            .broker()
            .complete_oauth(&owner, server.id(), &start.state, &consent.code)
            .await
            .expect_err("the flow ended");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        assert_eq!(f.requests().code_exchanges, 0);

        authorize(&f, &owner, server.id()).await;
        f.broker().lease(&owner, server.id()).await.expect("lease");
    })
    .await;
}

/// The `state` of another person's flow is `Unknown` to anyone else.
pub async fn another_owners_state_is_unknown<F: BrokerFixture>(f: F) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let server = add(&f, &alice, oauth()).await;
        let start = f
            .broker()
            .begin_oauth(&alice, server.id(), &f.redirect_uri())
            .await
            .expect("begin_oauth");
        let consent = f.consent(&start.authorize_url).await.expect("consent");

        let error = f
            .broker()
            .complete_oauth(&bob, server.id(), &start.state, &consent.code)
            .await
            .expect_err("bob completes alice's flow");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        assert_eq!(f.requests().code_exchanges, 0);

        f.broker()
            .complete_oauth(&alice, server.id(), &start.state, &consent.code)
            .await
            .expect("alice still can");
    })
    .await;
}

/// A new `begin_oauth` replaces the pending flow: the old `state` is no longer one.
pub async fn begin_oauth_replaces_a_pending_flow<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        let first = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("first");
        let second = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("second");
        assert_ne!(first.state, second.state);

        let old = f.consent(&first.authorize_url).await.expect("consent");
        let error = f
            .broker()
            .complete_oauth(&owner, server.id(), &first.state, &old.code)
            .await
            .expect_err("the replaced flow");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");

        let new = f.consent(&second.authorize_url).await.expect("consent");
        f.broker()
            .complete_oauth(&owner, server.id(), &second.state, &new.code)
            .await
            .expect("the new flow");
    })
    .await;
}

/// A code the authorization server refuses is `NeedsReauth`, and it spent the state.
pub async fn a_refused_code_needs_reauth_and_spends_the_state<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        let start = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("begin_oauth");
        let error = f
            .broker()
            .complete_oauth(
                &owner,
                server.id(),
                &start.state,
                &ToolSecret::new("not-a-code"),
            )
            .await
            .expect_err("a code nobody issued");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");

        let consent = f.consent(&start.authorize_url).await.expect("consent");
        let error = f
            .broker()
            .complete_oauth(&owner, server.id(), &start.state, &consent.code)
            .await
            .expect_err("the state was spent");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("no grant");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
    })
    .await;
}

/// An authorization server whose metadata does not list `S256` is refused: nothing is begun.
pub async fn an_authorization_server_without_s256_is_refused<F: BrokerFixture>(f: F) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        f.set_pkce_s256(false);

        let error = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect_err("no PKCE S256");
        assert!(
            matches!(error, CredentialError::Unavailable { .. }),
            "{error:?}"
        );
        assert!(
            f.secrets()
                .get(&owner, &SecretServer::pending_flow(server.id()))
                .await
                .expect("get")
                .is_none(),
            "no flow was begun"
        );

        f.set_pkce_s256(true);
        authorize(&f, &owner, server.id()).await;
    })
    .await;
}

/// `revoke` forgets the tokens and any pending flow, tells the authorization server where it can,
/// and is idempotent.
pub async fn revoke_forgets_tokens_and_pending_flow_and_tells_the_authorization_server<
    F: BrokerFixture,
>(
    f: F,
) {
    within(async {
        let owner = owner();
        let server = add(&f, &owner, oauth()).await;
        authorize(&f, &owner, server.id()).await;
        let pending = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("a second flow");
        let consent = f.consent(&pending.authorize_url).await.expect("consent");

        f.broker()
            .revoke(&owner, server.id())
            .await
            .expect("revoke");
        assert_eq!(f.requests().revocations, 1, "the grant is revoked upstream");

        let error = f
            .broker()
            .lease(&owner, server.id())
            .await
            .expect_err("revoked");
        assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
        for slot in [
            SecretServer::credential(server.id()),
            SecretServer::pending_flow(server.id()),
        ] {
            assert!(
                f.secrets().get(&owner, &slot).await.expect("get").is_none(),
                "{slot} is forgotten"
            );
        }
        let error = f
            .broker()
            .complete_oauth(&owner, server.id(), &pending.state, &consent.code)
            .await
            .expect_err("the flow was forgotten");
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");

        f.broker()
            .revoke(&owner, server.id())
            .await
            .expect("revoke is idempotent");
        assert_eq!(
            f.requests().revocations,
            1,
            "there was nothing left to revoke"
        );
    })
    .await;
}

// ----------------------------------------------------------------------------------- secrets

/// A planted secret never appears in an error or in a `Debug`: not the tokens, the codes, the
/// verifiers, the refresh tokens nor what the authorization server echoes in its errors.
pub async fn no_error_or_debug_text_shows_a_secret<F: BrokerFixture>(f: F) {
    within(async {
        const SENTINEL: &str = "SENTINEL-4f9c1e7a-do-not-print";
        const BEARER: &str = "SENTINEL-bearer-b3d1-do-not-print";
        f.plant(SENTINEL);
        let owner = owner();
        let mut errors: Vec<CredentialError> = Vec::new();
        let mut secrets: Vec<String> = vec![SENTINEL.to_owned(), BEARER.to_owned()];

        // a bearer server: the sentinel goes in and comes out only as a lease
        let bearer = add(&f, &owner, UserAuth::Bearer).await;
        f.broker()
            .store(&owner, bearer.id(), &ToolSecret::new(BEARER))
            .await
            .expect("store");
        let lease = f.broker().lease(&owner, bearer.id()).await.expect("lease");
        let shown = format!("{lease:?}");
        assert!(!shown.contains(BEARER), "a lease prints its token: {shown}");
        errors.extend(
            f.broker()
                .store(
                    &owner,
                    &UserToolServerId::generate(),
                    &ToolSecret::new(BEARER),
                )
                .await
                .err(),
        );

        // an OAuth server, through every way the authorization server can fail
        let server = add(&f, &owner, oauth()).await;
        let start = f
            .broker()
            .begin_oauth(&owner, server.id(), &f.redirect_uri())
            .await
            .expect("begin_oauth");
        let shown = format!("{start:?}");
        assert!(
            !shown.contains(&start.authorize_url) && !shown.contains(start.state.as_str()),
            "an OAuthStart prints the URL or the state: {shown}"
        );
        errors.extend(
            f.broker()
                .complete_oauth(
                    &owner,
                    server.id(),
                    &start.state,
                    &ToolSecret::new("bad-code"),
                )
                .await
                .err(),
        );
        authorize(&f, &owner, server.id()).await;
        let lease = f.broker().lease(&owner, server.id()).await.expect("lease");
        secrets.push(lease.secret().expose().to_owned());

        f.advance(f.access_ttl() + Duration::from_secs(1));
        for outcome in [RefreshOutcome::ServerError, RefreshOutcome::InvalidGrant] {
            f.set_refresh_outcome(outcome);
            errors.extend(f.broker().lease(&owner, server.id()).await.err());
        }
        f.set_refresh_outcome(RefreshOutcome::Succeeds);
        f.set_pkce_s256(false);
        errors.extend(
            f.broker()
                .begin_oauth(&owner, server.id(), &f.redirect_uri())
                .await
                .err(),
        );
        f.set_pkce_s256(true);

        secrets.extend(f.secrets_seen());
        assert!(secrets.len() > 4, "the authorization server saw secrets");
        assert!(!errors.is_empty(), "the cases provoked errors");
        for error in &errors {
            let said = everything_said_by(error);
            for secret in secrets.iter().filter(|s| s.len() >= 6) {
                assert!(
                    !said.contains(secret.as_str()),
                    "an error shows a secret: {said}"
                );
            }
        }
    })
    .await;
}
