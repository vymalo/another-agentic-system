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
async fn the_scopes_of_the_definition_are_asked_for_else_those_the_server_offers() {
    let fx = Fx::new().await;
    let (_, _, start) = begin(
        &fx,
        UserAuth::oauth2(vec!["repo".into(), "read:org".into()], None).unwrap(),
    )
    .await;
    assert_eq!(params(&start)["scope"], "repo read:org");
    let (_, _, start) = begin(&fx, UserAuth::oauth2(Vec::new(), None).unwrap()).await;
    assert_eq!(
        params(&start)["scope"],
        "read write",
        "the scopes_supported of the resource"
    );
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
