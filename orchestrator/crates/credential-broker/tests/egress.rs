//! The egress guard is in the path of every fetch of the broker (ADR 0046, decision 5): a URL the
//! guard refuses is not fetched, a name is resolved once and connected to only at the addresses
//! that passed, a redirect is not followed, and a response is bounded in size and time. Each test
//! asserts that **nothing connected** to what was refused, by a mock that counts requests or a
//! tripwire that counts connections.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::net::IpAddr;
use std::time::{Duration, Instant};

use orch_core::UserId;
use orch_credential_broker::{BrokerConfig, HostLookup, LookupFuture, egress_refusal};
use orch_ports::testkit::credential_broker::BrokerFixture;
use orch_ports::{
    CredentialBroker, CredentialError, EgressPolicy, EgressRefusal, UserAuth, UserToolServerId,
};
use support::{Fx, Tripwire, broker_over, owner, policy};

fn oauth() -> UserAuth {
    UserAuth::oauth2(Vec::new(), None).unwrap()
}

async fn begin(
    fx: &Fx,
    broker: &support::TestBroker,
    owner: &UserId,
    id: &UserToolServerId,
) -> Result<orch_ports::OAuthStart, CredentialError> {
    broker.begin_oauth(owner, id, &fx.redirect_uri()).await
}

/// Only requests for metadata reached the mock: nothing was asked of an endpoint it named.
fn only_metadata_was_fetched(fx: &Fx) {
    let seen = fx.mock.seen();
    assert!(
        seen.iter().all(|s| s.path.starts_with("/.well-known/")),
        "an endpoint was fetched: {seen:?}"
    );
}

async fn add_for(fx: &Fx, auth: UserAuth) -> (UserId, orch_ports::UserToolServer) {
    let who = owner();
    let server = fx.add(&who, auth).await;
    (who, server)
}

#[tokio::test]
async fn a_policy_that_does_not_name_the_loopback_address_refuses_every_fetch() {
    let fx = Fx::new().await;
    let (who, server) = add_for(&fx, oauth()).await;
    let strict = EgressPolicy::new().allowing_http();
    let broker = broker_over(
        &fx.secrets,
        &fx.servers,
        &fx.clock,
        strict,
        fx.config.clone(),
    );

    let error = begin(&fx, &broker, &who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::PrivateAddress));
    assert!(
        fx.mock.seen().is_empty(),
        "the mock was asked: {:?}",
        fx.mock.seen()
    );
}

#[tokio::test]
async fn http_is_refused_unless_the_policy_allows_it() {
    let fx = Fx::new().await;
    let (who, server) = add_for(&fx, oauth()).await;
    let https_only = EgressPolicy::new().with_allow("127.0.0.1".parse().unwrap());
    let broker = broker_over(
        &fx.secrets,
        &fx.servers,
        &fx.clock,
        https_only,
        fx.config.clone(),
    );

    let error = begin(&fx, &broker, &who, server.id()).await.unwrap_err();
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::Scheme));
    assert!(fx.mock.seen().is_empty());
}

#[tokio::test]
async fn a_token_endpoint_at_the_metadata_address_is_refused_before_any_connection() {
    let fx = Fx::new().await;
    fx.mock
        .configure(|c| c.token_endpoint = Some("http://169.254.169.254/latest/token".to_owned()));
    let (who, server) = add_for(&fx, oauth()).await;

    let error = begin(&fx, &fx.broker, &who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    // the policy names one host, so another is not allowed (the address is special besides)
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::NotAllowed));
    // the metadata that named it was fetched; nothing else was
    only_metadata_was_fetched(&fx);
    assert_eq!(
        fx.mock.seen().len(),
        2,
        "the resource's and the server's metadata"
    );
    // no flow was begun
    assert!(
        fx.broker
            .complete_oauth(
                &who,
                server.id(),
                &orch_ports::OAuthState::new("AAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
                &orch_ports::ToolSecret::new("c"),
                None
            )
            .await
            .is_err()
    );
}

/// The policy of the tests that also names the metadata address, so that the allow list does not
/// decide it: the class of the address does (link-local is refused whatever the lists say).
fn policy_that_names_the_metadata_address() -> EgressPolicy {
    policy().with_allow("169.254.169.254".parse().unwrap())
}

#[tokio::test]
async fn the_metadata_address_is_refused_by_what_it_is_even_when_the_allow_list_names_it() {
    // token endpoint and registration endpoint, each at 169.254.169.254, begin nothing and connect
    // to nothing: the refusal is `private-address` (the class of the address), not `allow-list`
    type Set = fn(&mut support::Cfg);
    let sets: [(&str, Set); 2] = [
        ("token", |c| {
            c.token_endpoint = Some("http://169.254.169.254/latest/token".to_owned());
        }),
        ("registration", |c| {
            c.registration_endpoint = Some("http://169.254.169.254/register".to_owned());
        }),
    ];
    for (name, set) in sets {
        let fx = Fx::new().await;
        fx.mock.configure(set);
        let (who, server) = add_for(&fx, oauth()).await;
        let broker = broker_over(
            &fx.secrets,
            &fx.servers,
            &fx.clock,
            policy_that_names_the_metadata_address(),
            fx.config.clone(),
        );
        let error = begin(&fx, &broker, &who, server.id()).await.unwrap_err();
        assert_eq!(
            egress_refusal(&error),
            Some(EgressRefusal::PrivateAddress),
            "{name}: {error:?}"
        );
        assert!(fx.mock.seen_on("/register").is_empty(), "{name}");
        assert!(fx.mock.seen_on("/authorize").is_empty(), "{name}");
    }
}

#[tokio::test]
async fn a_token_endpoint_at_a_private_host_the_policy_does_not_name_is_never_connected_to() {
    let fx = Fx::new().await;
    // 127.0.0.2 is loopback too, and the policy names only 127.0.0.1
    let tripwire = Tripwire::start_on("127.0.0.2").await;
    fx.mock.configure(|c| {
        c.token_endpoint = Some(format!("http://{}/token", tripwire.addr()));
    });
    let (who, server) = add_for(&fx, oauth()).await;

    let error = begin(&fx, &fx.broker, &who, server.id()).await.unwrap_err();
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::NotAllowed));
    assert_eq!(
        tripwire.connections(),
        0,
        "something connected to the refused host"
    );
}

#[tokio::test]
async fn a_registration_endpoint_the_policy_refuses_is_never_connected_to() {
    let fx = Fx::new().await;
    let tripwire = Tripwire::start_on("127.0.0.2").await;
    fx.mock.configure(|c| {
        c.registration_endpoint = Some(format!("http://{}/register", tripwire.addr()));
    });
    let (who, server) = add_for(&fx, oauth()).await;

    let error = begin(&fx, &fx.broker, &who, server.id()).await.unwrap_err();
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::NotAllowed));
    assert_eq!(tripwire.connections(), 0);
    assert!(fx.mock.seen_on("/register").is_empty());
}

#[tokio::test]
async fn a_revocation_endpoint_the_policy_refuses_is_never_connected_to_and_revoke_still_succeeds()
{
    let fx = Fx::new().await;
    let tripwire = Tripwire::start_on("127.0.0.2").await;
    fx.mock.configure(|c| {
        c.revocation_endpoint = Some(format!("http://{}/revoke", tripwire.addr()));
    });
    let (who, server) = add_for(&fx, oauth()).await;
    let start = begin(&fx, &fx.broker, &who, server.id()).await.unwrap();
    let consent = fx.consent(&start.authorize_url).await.unwrap();
    fx.broker
        .complete_oauth(
            &who,
            server.id(),
            &start.state,
            &consent.code,
            consent.iss.as_deref(),
        )
        .await
        .unwrap();

    fx.broker
        .revoke(&who, server.id())
        .await
        .expect("revocation upstream is best effort");
    assert_eq!(tripwire.connections(), 0);
    assert!(
        matches!(
            fx.broker.lease(&who, server.id()).await,
            Err(CredentialError::NeedsReauth)
        ),
        "the credential is gone all the same"
    );
}

#[tokio::test]
async fn the_policy_of_now_applies_at_a_refresh_not_the_policy_of_the_grant() {
    let fx = Fx::new().await;
    let (who, server) = add_for(&fx, oauth()).await;
    let start = begin(&fx, &fx.broker, &who, server.id()).await.unwrap();
    let consent = fx.consent(&start.authorize_url).await.unwrap();
    fx.broker
        .complete_oauth(
            &who,
            server.id(),
            &start.state,
            &consent.code,
            consent.iss.as_deref(),
        )
        .await
        .unwrap();
    fx.advance(fx.access_ttl() + Duration::from_secs(1));

    // the deployment's deny list now names the host
    let denying = policy().with_deny("127.0.0.1".parse().unwrap());
    let strict = broker_over(
        &fx.secrets,
        &fx.servers,
        &fx.clock,
        denying,
        fx.config.clone(),
    );
    let error = strict.lease(&who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::Denied));
    assert_eq!(
        fx.requests().refreshes,
        0,
        "the refused refresh was not sent"
    );

    // the grant is kept: the policy that allows it refreshes
    fx.broker.lease(&who, server.id()).await.expect("refreshes");
    assert_eq!(fx.requests().refreshes, 1);
}

#[tokio::test]
async fn an_authorization_endpoint_with_a_fragment_is_refused() {
    let fx = Fx::new().await;
    let origin = fx.mock.origin();
    fx.mock.configure(|c| {
        c.authorization_endpoint = Some(format!("{origin}/authorize#fragment"));
    });
    let (who, server) = add_for(&fx, oauth()).await;
    let error = begin(&fx, &fx.broker, &who, server.id()).await.unwrap_err();
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::Fragment));
}

#[tokio::test]
async fn an_authorization_endpoint_that_is_not_http_s_is_refused() {
    let fx = Fx::new().await;
    fx.mock
        .configure(|c| c.authorization_endpoint = Some("javascript:alert(1)".to_owned()));
    let (who, server) = add_for(&fx, oauth()).await;
    let error = begin(&fx, &fx.broker, &who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::Scheme));
}

// ----------------------------------------------------------------------------- the resolver

/// A resolver of the test's: every name resolves to these addresses, or fails.
struct Fixed(Result<Vec<IpAddr>, ()>);

impl HostLookup for Fixed {
    fn lookup<'a>(&'a self, _host: &'a str) -> LookupFuture<'a> {
        let answer = self.0.clone();
        Box::pin(async move { answer.map_err(|()| std::io::Error::other("the resolver is down")) })
    }
}

/// A broker whose resolver answers `answer` for every name, over a mock that calls itself
/// `rebind.test`.
async fn named(
    answer: Result<Vec<IpAddr>, ()>,
    policy: EgressPolicy,
) -> (Fx, support::TestBroker, UserId, UserToolServerId) {
    let fx = Fx::new().await;
    let port = fx.mock.addr().port();
    fx.mock
        .configure(|c| c.origin = Some(format!("http://rebind.test:{port}")));
    let who = owner();
    // added under the permissive policy: a name passes the guard before it is resolved
    let (server, _) = support::add_server_at(
        &fx.servers,
        &policy_for_names(),
        &who,
        oauth(),
        &format!("http://rebind.test:{port}/mcp"),
    )
    .await;
    let broker = orch_credential_broker::Broker::builder(
        support::Interleaved(fx.secrets.clone()),
        fx.paused.clone(),
        policy,
    )
    .clock(fx.clock.clone())
    .lookup(Fixed(answer))
    .build()
    .unwrap();
    let id = server.id().clone();
    (fx, broker, who, id)
}

fn policy_for_names() -> EgressPolicy {
    EgressPolicy::new().allowing_http()
}

#[tokio::test]
async fn a_name_that_resolves_to_a_private_address_is_refused_and_nothing_connects() {
    // the DNS rebinding case: a public-looking name, a loopback answer
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    let (fx, broker, who, id) = named(Ok(vec![loopback]), policy_for_names()).await;
    let error = begin(&fx, &broker, &who, &id).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::PrivateAddress));
    assert!(
        fx.mock.seen().is_empty(),
        "the mock was asked: {:?}",
        fx.mock.seen()
    );
}

#[tokio::test]
async fn one_refused_address_among_the_answers_refuses_the_whole_answer() {
    let public: IpAddr = "93.184.216.34".parse().unwrap();
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    let (fx, broker, who, id) = named(Ok(vec![public, loopback]), policy_for_names()).await;
    let error = begin(&fx, &broker, &who, &id).await.unwrap_err();
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::PrivateAddress));
    assert!(fx.mock.seen().is_empty());
}

#[tokio::test]
async fn a_name_that_resolves_to_nothing_is_refused() {
    let (fx, broker, who, id) = named(Ok(Vec::new()), policy_for_names()).await;
    let error = begin(&fx, &broker, &who, &id).await.unwrap_err();
    assert_eq!(egress_refusal(&error), Some(EgressRefusal::NoAddress));
}

#[tokio::test]
async fn a_resolver_that_fails_is_unavailable_and_not_a_refusal() {
    let (fx, broker, who, id) = named(Err(()), policy_for_names()).await;
    let error = begin(&fx, &broker, &who, &id).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(egress_refusal(&error), None);
    assert!(fx.mock.seen().is_empty());
}

#[tokio::test]
async fn the_connection_is_made_only_to_the_address_the_resolver_gave_and_the_guard_passed() {
    // the operator named the host exactly, so its loopback answer is admitted; the name does not
    // resolve in the system's resolver, so the connection can only have come from the hook
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    let policy = policy_for_names().with_allow("rebind.test".parse().unwrap());
    let (fx, broker, who, id) = named(Ok(vec![loopback]), policy).await;
    let start = begin(&fx, &broker, &who, &id)
        .await
        .expect("connects through the hook");
    assert!(start.authorize_url.starts_with("http://rebind.test:"));
    let paths: Vec<String> = fx.mock.seen().into_iter().map(|s| s.path).collect();
    assert_eq!(
        paths,
        [
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/oauth-authorization-server",
            "/register",
        ]
    );
}

// -------------------------------------------------------------------- redirects, size, time

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let fx = Fx::new().await;
    let tripwire = Tripwire::start_on("127.0.0.1").await;
    fx.mock.configure(|c| {
        c.redirect_metadata_to = Some(format!("http://{}/elsewhere", tripwire.addr()));
    });
    let (who, server) = add_for(&fx, oauth()).await;
    let error = begin(&fx, &fx.broker, &who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(tripwire.connections(), 0, "the redirect was followed");
}

#[tokio::test]
async fn a_response_over_the_limit_is_refused() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.metadata_padding = 80 * 1024);
    let (who, server) = add_for(&fx, oauth()).await;
    let error = begin(&fx, &fx.broker, &who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );

    // the limit is the broker's setting
    let roomy = BrokerConfig::default().with_max_response_bytes(256 * 1024);
    let broker = broker_over(&fx.secrets, &fx.servers, &fx.clock, policy(), roomy);
    begin(&fx, &broker, &who, server.id())
        .await
        .expect("within a larger limit");
}

#[tokio::test]
async fn a_response_over_the_limit_with_no_content_length_is_refused_as_it_arrives() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| {
        c.metadata_padding = 80 * 1024;
        c.metadata_chunks = Some(Duration::ZERO);
    });
    // the mock says nothing of the length up front: the body is chunked
    let origin = fx.mock.origin();
    let probe = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{origin}/.well-known/openid-configuration"))
        .send()
        .await
        .unwrap();
    assert_eq!(probe.content_length(), None, "a chunked body has no length");

    let (who, server) = add_for(&fx, oauth()).await;
    let error = begin(&fx, &fx.broker, &who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );

    // the same body is read within a larger limit
    let roomy = BrokerConfig::default().with_max_response_bytes(256 * 1024);
    let broker = broker_over(&fx.secrets, &fx.servers, &fx.clock, policy(), roomy);
    begin(&fx, &broker, &who, server.id())
        .await
        .expect("within a larger limit");
}

#[tokio::test]
async fn a_body_that_drips_is_cut_off_by_the_timeout_and_not_waited_for() {
    let fx = Fx::new().await;
    // ten chunks, a fifth of a second apart: two seconds, and the request may take 600 ms
    fx.mock.configure(|c| {
        c.metadata_padding = 40 * 1024;
        c.metadata_chunks = Some(Duration::from_millis(200));
    });
    let (who, server) = add_for(&fx, oauth()).await;
    let quick = BrokerConfig::default()
        .with_timeouts(Duration::from_millis(600), Duration::from_millis(600));
    let broker = broker_over(&fx.secrets, &fx.servers, &fx.clock, policy(), quick);

    let started = Instant::now();
    let error = begin(&fx, &broker, &who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "the body was waited for: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_slow_authorization_server_times_out() {
    let fx = Fx::new().await;
    fx.mock
        .configure(|c| c.metadata_delay = Duration::from_secs(3));
    let (who, server) = add_for(&fx, oauth()).await;
    let quick = BrokerConfig::default()
        .with_timeouts(Duration::from_millis(300), Duration::from_millis(300));
    let broker = broker_over(&fx.secrets, &fx.servers, &fx.clock, policy(), quick);

    let started = Instant::now();
    let error = begin(&fx, &broker, &who, server.id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}
