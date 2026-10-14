//! How the broker gets a client: the order of the ADR (a client id of the definition, a Client ID
//! Metadata Document, dynamic registration), and the authorization request it builds.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use orch_credential_broker::{BrokerConfig, BuildError};
use orch_ports::testkit::credential_broker::BrokerFixture;
use orch_ports::{CredentialBroker, CredentialError, OAuthStart, UserAuth};
use sha2::{Digest, Sha256};
use support::{Fx, owner, policy};

const DOCUMENT: &str = "https://orchestrator.test/oauth/client.json";

fn params(start: &OAuthStart) -> HashMap<String, String> {
    url::Url::parse(&start.authorize_url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

async fn begin(
    fx: &Fx,
    auth: UserAuth,
) -> (orch_core::UserId, orch_ports::UserToolServerId, OAuthStart) {
    let who = owner();
    let server = fx.add(&who, auth).await;
    let start = fx
        .broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .expect("begin_oauth");
    (who, server.id().clone(), start)
}

async fn complete(
    fx: &Fx,
    who: &orch_core::UserId,
    id: &orch_ports::UserToolServerId,
    start: &OAuthStart,
) {
    let consent = fx.consent(&start.authorize_url).await.expect("consent");
    fx.broker
        .complete_oauth(who, id, &start.state, &consent.code, consent.iss.as_deref())
        .await
        .expect("complete_oauth");
}

#[tokio::test]
async fn a_client_id_of_the_definition_is_used_and_nothing_is_registered() {
    let fx = Fx::new().await;
    fx.mock
        .configure(|c| c.pre_registered = Some("my-client".to_owned()));
    let (who, id, start) = begin(
        &fx,
        UserAuth::oauth2(Vec::new(), Some("my-client".into())).unwrap(),
    )
    .await;
    assert_eq!(params(&start)["client_id"], "my-client");
    assert!(
        fx.mock.seen_on("/register").is_empty(),
        "nothing was registered"
    );

    complete(&fx, &who, &id, &start).await;
    let exchange = &fx.mock.seen_on("/token")[0];
    assert_eq!(exchange.form("client_id"), Some("my-client"));
    assert!(
        exchange.authorization.is_none(),
        "a public client sends no secret"
    );
    assert!(exchange.form("client_secret").is_none());
}

#[tokio::test]
async fn the_clients_of_the_definition_win_over_a_document_the_server_would_accept() {
    let fx = Fx::with(BrokerConfig::default().with_client_metadata_url(DOCUMENT)).await;
    fx.mock.configure(|c| {
        c.pre_registered = Some("my-client".to_owned());
        c.cimd = true;
    });
    let (_, _, start) = begin(
        &fx,
        UserAuth::oauth2(Vec::new(), Some("my-client".into())).unwrap(),
    )
    .await;
    assert_eq!(params(&start)["client_id"], "my-client");
    assert!(fx.mock.seen_on("/register").is_empty());
}

#[tokio::test]
async fn a_client_id_metadata_document_is_used_when_the_server_supports_one_and_the_broker_has_one()
{
    let fx = Fx::with(BrokerConfig::default().with_client_metadata_url(DOCUMENT)).await;
    fx.mock.configure(|c| c.cimd = true);
    let (who, id, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    assert_eq!(params(&start)["client_id"], DOCUMENT);
    assert!(fx.mock.seen_on("/register").is_empty(), "no registration");

    complete(&fx, &who, &id, &start).await;
    assert_eq!(
        fx.mock.seen_on("/token")[0].form("client_id"),
        Some(DOCUMENT)
    );
    assert!(fx.broker.lease(&who, &id).await.is_ok());
}

#[tokio::test]
async fn without_a_document_url_the_broker_registers_even_if_the_server_supports_documents() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.cimd = true);
    let (_, _, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    assert_eq!(fx.mock.seen_on("/register").len(), 1);
    assert!(params(&start)["client_id"].starts_with("dcr-client-"));
}

#[tokio::test]
async fn a_server_without_documents_is_registered_with_even_if_the_broker_has_one() {
    let fx = Fx::with(BrokerConfig::default().with_client_metadata_url(DOCUMENT)).await;
    let (_, _, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    assert_eq!(fx.mock.seen_on("/register").len(), 1);
    assert_ne!(params(&start)["client_id"], DOCUMENT);
}

#[tokio::test]
async fn dynamic_registration_asks_for_a_public_client_for_the_redirect() {
    let fx = Fx::with(BrokerConfig::default().with_client_name("Orchestrator under test")).await;
    let (_, _, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    let registration = &fx.mock.seen_on("/register")[0];
    let body = registration.json.as_ref().expect("a JSON body");
    assert_eq!(body["client_name"], "Orchestrator under test");
    assert_eq!(
        body["redirect_uris"],
        serde_json::json!([fx.redirect_uri().as_str()])
    );
    assert_eq!(body["token_endpoint_auth_method"], "none");
    assert_eq!(body["response_types"], serde_json::json!(["code"]));
    assert_eq!(
        body["grant_types"],
        serde_json::json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(params(&start)["redirect_uri"], fx.redirect_uri().as_str());
}

#[tokio::test]
async fn a_client_secret_from_registration_authenticates_the_token_and_the_refresh_requests() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.registration_secret = true);
    let (who, id, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    complete(&fx, &who, &id, &start).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.broker
        .lease(&who, &id)
        .await
        .expect("a refresh the server authenticated");

    let client_id = params(&start)["client_id"].clone();
    let requests = fx.mock.seen_on("/token");
    assert_eq!(requests.len(), 2);
    for request in requests {
        let header = request.authorization.clone().expect("client_secret_basic");
        let decoded = String::from_utf8(
            STANDARD
                .decode(header.strip_prefix("Basic ").unwrap())
                .unwrap(),
        )
        .unwrap();
        let (id_part, secret) = decoded.split_once(':').unwrap();
        assert_eq!(id_part, client_id);
        assert!(secret.starts_with("dcr-secret-"), "{secret}");
        assert!(
            request.form("client_secret").is_none(),
            "not in the body as well"
        );
    }
}

#[tokio::test]
async fn no_client_id_and_no_way_to_register_is_unavailable() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.registration = false);
    let who = owner();
    let server = fx
        .add(&who, UserAuth::oauth2(Vec::new(), None).unwrap())
        .await;
    let error = fx
        .broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert!(fx.mock.seen_on("/authorize").is_empty());
}

#[tokio::test]
async fn the_authorization_request_carries_pkce_state_resource_and_the_redirect() {
    let fx = Fx::new().await;
    let (who, id, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    let p = params(&start);
    assert_eq!(p["response_type"], "code");
    assert_eq!(p["code_challenge_method"], "S256");
    assert_eq!(p["state"], start.state.as_str());
    assert!(p["state"].len() >= 43, "256 bits of state");
    assert_eq!(p["resource"], fx.mock.mcp_url());
    assert_eq!(p["redirect_uri"], fx.redirect_uri().as_str());
    assert!(
        start
            .authorize_url
            .starts_with(&format!("{}/authorize?", fx.mock.origin()))
    );

    // the challenge is that of the verifier the token request carries
    complete(&fx, &who, &id, &start).await;
    let exchange = &fx.mock.seen_on("/token")[0];
    let verifier = exchange.form("code_verifier").expect("a verifier");
    assert!((43..=128).contains(&verifier.len()));
    assert_eq!(
        p["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    );
    assert!(
        !start.authorize_url.contains(verifier),
        "the verifier is not in the URL"
    );
    assert_eq!(exchange.form("resource"), Some(fx.mock.mcp_url().as_str()));
    assert_eq!(
        exchange.form("redirect_uri"),
        Some(fx.redirect_uri().as_str())
    );
    assert_eq!(exchange.form("grant_type"), Some("authorization_code"));
}

#[tokio::test]
async fn states_and_verifiers_are_not_repeated() {
    let fx = Fx::new().await;
    let who = owner();
    let server = fx
        .add(&who, UserAuth::oauth2(Vec::new(), None).unwrap())
        .await;
    let mut states = Vec::new();
    let mut challenges = Vec::new();
    for _ in 0..5 {
        let start = fx
            .broker
            .begin_oauth(&who, server.id(), &fx.redirect_uri())
            .await
            .unwrap();
        let p = params(&start);
        states.push(p["state"].clone());
        challenges.push(p["code_challenge"].clone());
    }
    states.sort();
    states.dedup();
    challenges.sort();
    challenges.dedup();
    assert_eq!((states.len(), challenges.len()), (5, 5));
}

#[tokio::test]
async fn the_scopes_of_the_definition_are_asked_for_and_else_none() {
    let fx = Fx::new().await;
    let (_, _, start) = begin(
        &fx,
        UserAuth::oauth2(vec!["repo".into(), "read:org".into()], None).unwrap(),
    )
    .await;
    assert_eq!(params(&start)["scope"], "repo read:org");

    // the server's metadata lists `read` and `write`, and the person named none: the list is a
    // stranger's, and nobody has agreed to it
    let resource = fx.mock.seen_on("/.well-known/oauth-protected-resource/mcp");
    assert!(!resource.is_empty());
    let (_, _, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    assert!(
        !params(&start).contains_key("scope"),
        "no scope is asked for: {}",
        start.authorize_url
    );
}

#[tokio::test]
async fn the_resource_sent_is_the_definitions_url_and_not_the_documents_spelling_of_it() {
    let fx = Fx::new().await;
    // the document spells the scheme in capitals: the same resource, by `same_resource`
    let port = fx.mock.addr().port();
    fx.mock
        .configure(|c| c.declared_resource = Some(format!("HTTP://127.0.0.1:{port}/mcp")));
    let (who, id, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    assert_eq!(params(&start)["resource"], fx.mock.mcp_url());
    complete(&fx, &who, &id, &start).await;
    assert_eq!(
        fx.mock.seen_on("/token")[0].form("resource"),
        Some(fx.mock.mcp_url().as_str())
    );
}

#[tokio::test]
async fn two_begin_oauth_calls_register_one_client() {
    let fx = Fx::new().await;
    let who = owner();
    let server = fx
        .add(&who, UserAuth::oauth2(Vec::new(), None).unwrap())
        .await;
    let begin = |redirect: orch_ports::RedirectUri| {
        let (fx, who, id) = (&fx, who.clone(), server.id().clone());
        async move { fx.broker.begin_oauth(&who, &id, &redirect).await.unwrap() }
    };

    let first = begin(fx.redirect_uri()).await;
    let second = begin(fx.redirect_uri()).await;
    assert_eq!(fx.mock.seen_on("/register").len(), 1, "one registration");
    assert_eq!(params(&first)["client_id"], params(&second)["client_id"]);
    assert_ne!(first.state, second.state, "two flows all the same");

    // the client of a grant serves a person who authorizes again
    complete(&fx, &who, server.id(), &second).await;
    let third = begin(fx.redirect_uri()).await;
    assert_eq!(fx.mock.seen_on("/register").len(), 1);
    assert_eq!(params(&third)["client_id"], params(&first)["client_id"]);

    // a client is registered for one redirect URI: another redirect is another client
    let elsewhere =
        begin(orch_ports::RedirectUri::parse("https://orchestrator.test/other/callback").unwrap())
            .await;
    assert_eq!(fx.mock.seen_on("/register").len(), 2);
    assert_ne!(params(&elsewhere)["client_id"], params(&first)["client_id"]);

    // and a registration is not trusted for ever: the server may have forgotten it
    fx.advance(Duration::from_secs(3601));
    let later = begin(fx.redirect_uri()).await;
    assert_eq!(fx.mock.seen_on("/register").len(), 3);
    assert_ne!(params(&later)["client_id"], params(&first)["client_id"]);
}

#[tokio::test]
async fn a_client_registered_at_one_authorization_server_is_not_offered_to_another() {
    // a client with a secret: reusing it at another server would send that server the secret
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.registration_secret = true);
    let who = owner();
    let server = fx
        .add(&who, UserAuth::oauth2(Vec::new(), None).unwrap())
        .await;
    fx.broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .unwrap();
    assert_eq!(fx.mock.seen_on("/register").len(), 1);

    // the server's metadata now names another authorization server
    let other = support::MockAs::start().await;
    fx.mock
        .configure(|c| c.authorization_server = Some(other.origin()));
    fx.broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .unwrap();
    assert_eq!(
        fx.mock.seen_on("/register").len(),
        1,
        "the first server was not asked again"
    );
    assert_eq!(
        other.seen_on("/register").len(),
        1,
        "the other authorization server registered a client of its own"
    );
}

#[tokio::test]
async fn a_flow_that_cannot_be_kept_is_refused_before_a_client_is_registered() {
    // a long URL, the most scopes and long endpoints leave the sealed flow just under the 16 KiB the
    // store keeps; a long redirect URI is kept twice (the flow's and the registration's), and the
    // flow is over it if the client is the largest one a registration can give
    for (redirect_len, kept) in [(150, true), (800, false)] {
        let fx = Fx::new().await;
        let origin = fx.mock.origin();
        let long = format!("{origin}/{}", "a".repeat(1900));
        fx.mock.configure(|c| {
            c.root_resource = Some(long.clone());
            c.token_endpoint = Some(format!("{origin}/t{}", "a".repeat(1900)));
            c.revocation_endpoint = Some(format!("{origin}/r{}", "a".repeat(1900)));
        });
        let scopes: Vec<String> = (0..32)
            .map(|n| format!("{n:0>2}{}", "s".repeat(126)))
            .collect();
        let who = owner();
        let (server, _) = support::add_server_at(
            &fx.servers,
            &policy(),
            &who,
            UserAuth::oauth2(scopes, None).unwrap(),
            &long,
        )
        .await;
        let redirect = orch_ports::RedirectUri::parse(&format!(
            "https://orchestrator.test/cb/{}",
            "a".repeat(redirect_len)
        ))
        .unwrap();
        let result = fx.broker.begin_oauth(&who, server.id(), &redirect).await;
        if kept {
            result.expect("a flow that fits is begun");
            assert_eq!(fx.mock.seen_on("/register").len(), 1);
        } else {
            let error = result.unwrap_err();
            assert!(
                matches!(error, CredentialError::Unavailable { .. }),
                "{error:?}"
            );
            assert!(
                fx.mock.seen_on("/register").is_empty(),
                "the client of a flow that was never begun was registered all the same"
            );
        }
    }
}

#[tokio::test]
async fn an_authorization_request_over_8_kib_is_refused_before_a_client_is_registered() {
    let fx = Fx::new().await;
    // 32 scopes of 128 bytes, the resource and an authorization endpoint of 2 KiB
    let origin = fx.mock.origin();
    let long = format!("{origin}/{}", "a".repeat(1900));
    fx.mock.configure(|c| {
        c.root_resource = Some(long.clone());
        c.authorization_endpoint = Some(format!("{origin}/a{}", "a".repeat(1950)));
    });
    let scopes: Vec<String> = (0..32)
        .map(|n| format!("{n:0>2}{}", "s".repeat(126)))
        .collect();
    let who = owner();
    let (server, _) = support::add_server_at(
        &fx.servers,
        &policy(),
        &who,
        UserAuth::oauth2(scopes, None).unwrap(),
        &long,
    )
    .await;
    let error = fx
        .broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert!(fx.mock.seen_on("/register").is_empty());
}

#[tokio::test]
async fn a_url_over_2_kib_in_the_metadata_is_refused() {
    type Set = fn(&mut support::Cfg, String);
    let sets: [(&str, Set); 4] = [
        ("authorization", |c, u| c.authorization_endpoint = Some(u)),
        ("token", |c, u| c.token_endpoint = Some(u)),
        ("registration", |c, u| c.registration_endpoint = Some(u)),
        ("revocation", |c, u| c.revocation_endpoint = Some(u)),
    ];
    for (name, set) in sets {
        let fx = Fx::new().await;
        let long = format!("{}/{}", fx.mock.origin(), "a".repeat(2100));
        fx.mock.configure(|c| set(c, long));
        let who = owner();
        let server = fx
            .add(&who, UserAuth::oauth2(Vec::new(), None).unwrap())
            .await;
        let error = fx
            .broker
            .begin_oauth(&who, server.id(), &fx.redirect_uri())
            .await
            .unwrap_err();
        assert!(
            matches!(error, CredentialError::Unavailable { .. }),
            "{name}: {error:?}"
        );
        // refused as the metadata says it, with a phrase of the broker's and not the guard's
        assert_eq!(
            orch_credential_broker::egress_refusal(&error),
            None,
            "{name}: refused for its size before the guard saw it"
        );
        assert!(fx.mock.seen_on("/register").is_empty(), "{name}");
        assert!(fx.mock.seen_on("/token").is_empty(), "{name}");
    }
}

#[tokio::test]
async fn the_flow_ends_when_the_configuration_says() {
    let fx = Fx::with(BrokerConfig::default().with_flow_ttl(Duration::from_secs(90))).await;
    let before = orch_ports::testkit::credential_broker::BrokerFixture::now(&fx);
    let (_, _, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    assert_eq!(start.expires_at.as_second() - before.as_second(), 90);
}

#[tokio::test]
async fn the_resource_of_the_server_is_discovered_at_the_root_when_the_path_has_none() {
    let fx = Fx::new().await;
    let legacy = format!("{}/legacy", fx.mock.origin());
    fx.mock
        .configure(|c| c.root_resource = Some(legacy.clone()));
    let who = owner();
    let (server, _) = support::add_server_at(
        &fx.servers,
        &policy(),
        &who,
        UserAuth::oauth2(Vec::new(), None).unwrap(),
        &legacy,
    )
    .await;
    let start = fx
        .broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .expect("the root metadata is tried after the path's");
    assert_eq!(params(&start)["resource"], legacy);
    let paths: Vec<String> = fx.mock.seen().into_iter().map(|s| s.path).collect();
    assert_eq!(
        &paths[..2],
        [
            "/.well-known/oauth-protected-resource/legacy",
            "/.well-known/oauth-protected-resource"
        ]
    );
}

#[tokio::test]
async fn an_issuer_that_publishes_only_openid_configuration_is_found() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.oidc_only = true);
    let (_, _, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    assert!(start.authorize_url.contains("code_challenge="));
    let paths: Vec<String> = fx.mock.seen().into_iter().map(|s| s.path).collect();
    assert!(paths.contains(&"/.well-known/oauth-authorization-server".to_owned()));
    assert!(paths.contains(&"/.well-known/openid-configuration".to_owned()));
}

#[tokio::test]
async fn metadata_that_is_about_another_resource_or_another_issuer_is_not_used() {
    for (declared_resource, declared_issuer) in [
        (Some("https://evil.example/mcp".to_owned()), None),
        (None, Some("https://evil.example".to_owned())),
    ] {
        let fx = Fx::new().await;
        fx.mock.configure(|c| {
            c.declared_resource = declared_resource;
            c.declared_issuer = declared_issuer;
        });
        let who = owner();
        let server = fx
            .add(&who, UserAuth::oauth2(Vec::new(), None).unwrap())
            .await;
        let error = fx
            .broker
            .begin_oauth(&who, server.id(), &fx.redirect_uri())
            .await
            .unwrap_err();
        assert!(
            matches!(error, CredentialError::Unavailable { .. }),
            "{error:?}"
        );
        assert!(fx.mock.seen_on("/authorize").is_empty());
        assert!(
            fx.mock.seen_on("/register").is_empty(),
            "nothing was registered with it"
        );
    }
}

#[tokio::test]
async fn a_token_that_is_not_a_bearer_token_is_not_kept() {
    let fx = Fx::new().await;
    fx.mock
        .configure(|c| c.token_type = Some("DPoP".to_owned()));
    let (who, id, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    let consent = fx.consent(&start.authorize_url).await.unwrap();
    let error = fx
        .broker
        .complete_oauth(
            &who,
            &id,
            &start.state,
            &consent.code,
            consent.iss.as_deref(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
    assert!(matches!(
        fx.broker.lease(&who, &id).await,
        Err(CredentialError::NeedsReauth)
    ));
}

#[test]
fn the_settings_are_checked_when_the_broker_is_built() {
    let build = |config: BrokerConfig| {
        orch_credential_broker::Broker::builder(
            orch_ports::memory::MemorySecretStore::new(),
            orch_ports::memory::MemoryUserToolServers::new(),
            policy(),
        )
        .config(config)
        .build()
        .map(|_| ())
    };
    build(BrokerConfig::default()).expect("the defaults");
    build(BrokerConfig::default().with_client_metadata_url(DOCUMENT)).expect("a document URL");
    for bad in [
        "http://orchestrator.test/client.json",
        "https://orchestrator.test/",
        "https://orchestrator.test",
        "https://orchestrator.test/client.json#frag",
        "https://user:pw@orchestrator.test/client.json",
        "not a url",
    ] {
        assert!(
            matches!(
                build(BrokerConfig::default().with_client_metadata_url(bad)),
                Err(BuildError::ClientMetadataUrl)
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        build(BrokerConfig::default().with_lease_ttl(Duration::ZERO)),
        Err(BuildError::Config)
    ));
    assert!(matches!(
        build(BrokerConfig::default().with_max_response_bytes(0)),
        Err(BuildError::Config)
    ));
    assert!(matches!(
        build(BrokerConfig::default().with_client_name("  ")),
        Err(BuildError::Config)
    ));
}

#[test]
fn every_duration_and_the_response_size_have_an_upper_bound() {
    use orch_credential_broker::{
        MAX_FLOW_TTL, MAX_LEASE_TTL, MAX_REFRESH_MARGIN, MAX_RESPONSE_BYTES, MAX_TIMEOUT,
    };
    let build = |config: BrokerConfig| {
        orch_credential_broker::Broker::builder(
            orch_ports::memory::MemorySecretStore::new(),
            orch_ports::memory::MemoryUserToolServers::new(),
            policy(),
        )
        .config(config)
        .build()
        .map(|_| ())
    };
    let more = |d: Duration| d + Duration::from_secs(1);
    // the bound itself is allowed, one more is not
    build(BrokerConfig::default().with_lease_ttl(MAX_LEASE_TTL)).expect("an hour");
    build(BrokerConfig::default().with_flow_ttl(MAX_FLOW_TTL)).expect("an hour");
    build(BrokerConfig::default().with_refresh_margin(MAX_REFRESH_MARGIN)).expect("ten minutes");
    build(BrokerConfig::default().with_timeouts(MAX_TIMEOUT, MAX_TIMEOUT)).expect("a minute");
    build(BrokerConfig::default().with_max_response_bytes(MAX_RESPONSE_BYTES)).expect("1 MiB");
    for (what, config) in [
        (
            "lease",
            BrokerConfig::default().with_lease_ttl(more(MAX_LEASE_TTL)),
        ),
        (
            "flow",
            BrokerConfig::default().with_flow_ttl(more(MAX_FLOW_TTL)),
        ),
        (
            "margin",
            BrokerConfig::default().with_refresh_margin(more(MAX_REFRESH_MARGIN)),
        ),
        (
            "request",
            BrokerConfig::default().with_timeouts(more(MAX_TIMEOUT), Duration::from_secs(1)),
        ),
        (
            "connect",
            BrokerConfig::default().with_timeouts(Duration::from_secs(1), more(MAX_TIMEOUT)),
        ),
        (
            "response",
            BrokerConfig::default().with_max_response_bytes(MAX_RESPONSE_BYTES + 1),
        ),
    ] {
        assert!(
            matches!(build(config), Err(BuildError::Config)),
            "{what} is above its bound"
        );
    }
    let long = format!("https://orchestrator.test/{}", "a".repeat(2100));
    assert!(matches!(
        build(BrokerConfig::default().with_client_metadata_url(long)),
        Err(BuildError::ClientMetadataUrl)
    ));
}
