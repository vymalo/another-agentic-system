//! No secret reaches a log line (ADR 0046): the whole life of a credential runs under a subscriber
//! that keeps every event of every level, with a sentinel planted in everything the authorization
//! server issues and echoes, and the log is searched for it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_ports::testkit::credential_broker::{BrokerFixture, RefreshOutcome};
use orch_ports::{CredentialBroker, EgressPolicy, ToolSecret, UserAuth};
use support::{Captured, Fx, broker_over, owner};

// one thread runs the test, the mock and the spawned refresh, so the subscriber sees them all
#[tokio::test(flavor = "current_thread")]
async fn no_log_line_shows_a_secret_or_a_url() {
    let (log, _guard) = Captured::install();

    const SENTINEL: &str = "SENTINEL-77c3a1-do-not-log";
    let fx = Fx::new().await;
    fx.plant(SENTINEL);
    fx.mock.configure(|c| {
        c.registration_secret = true;
    });
    let who = owner();
    let server = fx
        .add(&who, UserAuth::oauth2(Vec::new(), None).unwrap())
        .await;
    let bearer = fx.add(&who, UserAuth::Bearer).await;
    fx.broker
        .store(&who, bearer.id(), &ToolSecret::new("SENTINEL-bearer-typed"))
        .await
        .unwrap();

    // a whole authorization, then the ways it ends
    let start = fx
        .broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .unwrap();
    let consent = fx.consent(&start.authorize_url).await.unwrap();
    fx.broker
        .complete_oauth(
            &who,
            server.id(),
            &start.state,
            &ToolSecret::new("wrong-code"),
            Some(&start.issuer),
        )
        .await
        .unwrap_err();
    let start = fx
        .broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .unwrap();
    let consent2 = fx.consent(&start.authorize_url).await.unwrap();
    let _ = consent;
    fx.broker
        .complete_oauth(
            &who,
            server.id(),
            &start.state,
            &consent2.code,
            consent2.iss.as_deref(),
        )
        .await
        .unwrap();
    fx.broker.lease(&who, server.id()).await.unwrap();
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.set_refresh_outcome(RefreshOutcome::ServerError);
    fx.broker.lease(&who, server.id()).await.unwrap_err();
    fx.set_refresh_outcome(RefreshOutcome::InvalidGrant);
    fx.broker.lease(&who, server.id()).await.unwrap_err();
    // and a refusal of the egress guard
    let strict = broker_over(
        &fx.secrets,
        &fx.servers,
        &fx.clock,
        EgressPolicy::new().allowing_http(),
        fx.config.clone(),
    );
    strict
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .unwrap_err();
    fx.broker.revoke(&who, server.id()).await.unwrap();
    fx.broker.revoke(&who, bearer.id()).await.unwrap();

    let text = log.text();
    assert!(
        text.contains("refused"),
        "the log should hold the warnings of the refusals: {text}"
    );
    let mut secrets = fx.secrets_seen();
    secrets.push(SENTINEL.to_owned());
    secrets.push("SENTINEL-bearer-typed".to_owned());
    secrets.push("wrong-code".to_owned());
    assert!(secrets.len() > 6);
    for secret in secrets.iter().filter(|s| s.len() >= 6) {
        assert!(
            !text.contains(secret.as_str()),
            "the log shows `{secret}`:\n{text}"
        );
    }
    // not the server's URL either: its path can carry a secret
    assert!(
        !text.contains("/mcp"),
        "the log shows the server's path:\n{text}"
    );
}
