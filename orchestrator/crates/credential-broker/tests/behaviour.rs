//! What this broker does beyond the port's conformance suite: the shape of what it keeps, the
//! tokens' lifetimes, the state it writes, revocation upstream, stores that are down, and a refresh
//! that outlives its caller.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::error::Error as _;
use std::time::Duration;

use futures::future::join_all;
use orch_credential_broker::{Broker, BrokerConfig};
use orch_ports::memory::MemorySecretStore;
use orch_ports::testkit::credential_broker::{BrokerFixture, RefreshOutcome};
use orch_ports::{
    Clock, CredentialBroker, CredentialError, SecretError, SecretMaterial, SecretServer,
    SecretStore, ToolSecret, UserAuth, UserToolServer, UserToolServerId, UserToolServerState,
    UserToolServerStore,
};
use serde_json::json;
use support::{FailRotate, Fx, Interleaved, owner, policy};

fn oauth() -> UserAuth {
    UserAuth::oauth2(Vec::new(), None).unwrap()
}

/// A server of the mock that has been authorized.
async fn authorized(fx: &Fx) -> (orch_core::UserId, UserToolServerId) {
    let who = owner();
    let server = fx.add(&who, oauth()).await;
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
            &consent.code,
            consent.iss.as_deref(),
        )
        .await
        .unwrap();
    (who, server.id().clone())
}

async fn kept(fx: &Fx, who: &orch_core::UserId, id: &UserToolServerId) -> String {
    let (_, material) = fx
        .secrets
        .get(who, &SecretServer::credential(id))
        .await
        .unwrap()
        .expect("a credential is kept");
    String::from_utf8(material.expose().to_vec()).unwrap()
}

/// Edits the grant kept for the server, as a replica of the broker would have written it: `put`,
/// so the version moves.
async fn edit_grant(
    fx: &Fx,
    who: &orch_core::UserId,
    id: &UserToolServerId,
    edit: impl FnOnce(&mut serde_json::Value),
) {
    let slot = SecretServer::credential(id);
    let (_, material) = fx.secrets.get(who, &slot).await.unwrap().unwrap();
    let mut doc: serde_json::Value = serde_json::from_slice(material.expose()).unwrap();
    edit(&mut doc["d"]["oauth2"]);
    let edited = SecretMaterial::new(serde_json::to_vec(&doc).unwrap()).unwrap();
    fx.secrets.put(who, &slot, &edited).await.unwrap();
}

async fn access_token(fx: &Fx, who: &orch_core::UserId, id: &UserToolServerId) -> String {
    let text = kept(fx, who, id).await;
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    doc["d"]["oauth2"]["tokens"]["access"]
        .as_str()
        .unwrap()
        .to_owned()
}

// ------------------------------------------------------------------------------------- store

#[tokio::test]
async fn store_is_for_the_kinds_a_person_types_a_secret_for() {
    let fx = Fx::new().await;
    let who = owner();
    for auth in [UserAuth::None, oauth()] {
        let server = fx.add(&who, auth).await;
        let error = fx
            .broker
            .store(&who, server.id(), &ToolSecret::new("typed"))
            .await
            .unwrap_err();
        assert!(matches!(error, CredentialError::Unknown), "{error:?}");
        assert!(
            fx.secrets
                .get(&who, &SecretServer::credential(server.id()))
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn what_is_kept_names_the_url_it_was_issued_for() {
    let fx = Fx::new().await;
    let who = owner();
    let server = fx.add(&who, UserAuth::Bearer).await;
    fx.broker
        .store(&who, server.id(), &ToolSecret::new("a-bearer-token"))
        .await
        .unwrap();
    let text = kept(&fx, &who, server.id()).await;
    assert!(text.contains(server.url()), "{text}");

    let (who, id) = authorized(&fx).await;
    let text = kept(&fx, &who, &id).await;
    assert!(
        text.contains(&fx.mock.mcp_url()),
        "the URL is kept with the grant: {text}"
    );
    assert!(text.contains("/token"), "so are the endpoints of the grant");
}

#[tokio::test]
async fn a_credential_that_does_not_fit_the_store_is_unavailable() {
    let fx = Fx::new().await;
    let who = owner();
    let server = fx.add(&who, UserAuth::Bearer).await;
    let error = fx
        .broker
        .store(&who, server.id(), &ToolSecret::new("x".repeat(20 * 1024)))
        .await
        .unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_credential_of_another_kind_than_the_definitions_needs_reauth() {
    let fx = Fx::new().await;
    let who = owner();
    let (server, revision) = support::add_server_at(
        &fx.servers,
        &policy(),
        &who,
        UserAuth::Bearer,
        &fx.mock.mcp_url(),
    )
    .await;
    fx.broker
        .store(&who, server.id(), &ToolSecret::new("a-token"))
        .await
        .unwrap();
    // the same URL, now a key in a header
    let target = policy().check_url(&fx.mock.mcp_url()).unwrap();
    let edited = UserToolServer::new(
        server.id().clone(),
        "Keyed",
        &target,
        UserAuth::api_key("X-Api-Key").unwrap(),
    )
    .unwrap();
    fx.servers.replace(&who, &edited, revision).await.unwrap();
    let error = fx.broker.lease(&who, server.id()).await.unwrap_err();
    assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
}

#[tokio::test]
async fn what_cannot_be_read_back_is_unavailable_and_not_a_request_to_sign_in() {
    let fx = Fx::new().await;
    let who = owner();
    let server = fx.add(&who, UserAuth::Bearer).await;
    for garbage in [
        &b"not json"[..],
        br#"{"v":9,"d":{"bearer":{"url":"u","token":"t"}}}"#,
        br#"{"v":1,"d":{"unheard_of":{}}}"#,
    ] {
        fx.secrets
            .put(
                &who,
                &SecretServer::credential(server.id()),
                &SecretMaterial::new(garbage.to_vec()).unwrap(),
            )
            .await
            .unwrap();
        let error = fx.broker.lease(&who, server.id()).await.unwrap_err();
        assert!(
            matches!(error, CredentialError::Unavailable { .. }),
            "{error:?}"
        );
    }
}

// ---------------------------------------------------------------------------------- lifetimes

#[tokio::test]
async fn a_lease_is_as_short_as_the_configuration_says_and_never_outlives_the_token() {
    let fx = Fx::with(BrokerConfig::default().with_lease_ttl(Duration::from_secs(120))).await;
    let who = owner();
    let bearer = fx.add(&who, UserAuth::Bearer).await;
    fx.broker
        .store(&who, bearer.id(), &ToolSecret::new("t"))
        .await
        .unwrap();
    let lease = fx.broker.lease(&who, bearer.id()).await.unwrap();
    assert_eq!(lease.expires_at().as_second(), fx.now().as_second() + 120);

    let (who, id) = authorized(&fx).await;
    let lease = fx.broker.lease(&who, &id).await.unwrap();
    assert_eq!(
        lease.expires_at().as_second(),
        fx.now().as_second() + 120,
        "capped by the lease"
    );
    // 60 seconds are left of the token: the lease ends with it
    fx.advance(fx.access_ttl() - Duration::from_secs(60));
    let lease = fx.broker.lease(&who, &id).await.unwrap();
    assert_eq!(lease.expires_at().as_second(), fx.now().as_second() + 60);
    assert_eq!(fx.requests().refreshes, 0);
}

#[tokio::test]
async fn a_server_that_gives_no_expiry_and_a_refresh_token_is_asked_again_after_an_hour() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.no_expires_in = true);
    let (who, id) = authorized(&fx).await;
    fx.advance(Duration::from_secs(3500));
    fx.broker.lease(&who, &id).await.unwrap();
    assert_eq!(fx.requests().refreshes, 0);
    fx.advance(Duration::from_secs(80));
    fx.broker.lease(&who, &id).await.unwrap();
    assert_eq!(fx.requests().refreshes, 1);
}

#[tokio::test]
async fn a_server_that_says_zero_seconds_is_read_as_a_server_that_said_nothing() {
    // `expires_in: 0` is sent by servers whose tokens do not expire; read as "ends now" it would
    // make every lease a refresh
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.ttl_secs = 0);
    let (who, id) = authorized(&fx).await;
    fx.broker.lease(&who, &id).await.unwrap();
    assert_eq!(
        fx.requests().refreshes,
        0,
        "not stale the moment it is made"
    );
    fx.advance(Duration::from_secs(3500));
    fx.broker.lease(&who, &id).await.unwrap();
    assert_eq!(fx.requests().refreshes, 0);
    fx.advance(Duration::from_secs(80));
    fx.broker.lease(&who, &id).await.unwrap();
    assert_eq!(
        fx.requests().refreshes,
        1,
        "an hour, as when nothing is said"
    );
}

#[tokio::test]
async fn a_server_that_gives_neither_expiry_nor_refresh_token_is_not_asked_again() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| {
        c.no_expires_in = true;
        c.no_refresh_token = true;
    });
    let (who, id) = authorized(&fx).await;
    fx.advance(Duration::from_secs(365 * 24 * 3600));
    let lease = fx.broker.lease(&who, &id).await.unwrap();
    assert!(!lease.secret().expose().is_empty());
    assert_eq!(fx.requests().refreshes, 0);
}

#[tokio::test]
async fn an_expired_token_with_no_refresh_token_needs_reauth_and_asks_nobody() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.no_refresh_token = true);
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    let error = fx.broker.lease(&who, &id).await.unwrap_err();
    assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
    assert_eq!(fx.requests().refreshes, 0);
    let stored = fx.servers.get(&who, &id).await.unwrap().unwrap();
    assert_eq!(stored.server().state(), UserToolServerState::NeedsReauth);
}

#[tokio::test]
async fn a_refresh_token_the_server_rotates_is_kept_and_used_next() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.rotate_refresh = true);
    let (who, id) = authorized(&fx).await;
    for _ in 0..2 {
        fx.advance(fx.access_ttl() + Duration::from_secs(1));
        fx.broker.lease(&who, &id).await.unwrap();
    }
    let used: Vec<String> = fx
        .mock
        .seen_on("/token")
        .iter()
        .filter(|s| s.form("grant_type") == Some("refresh_token"))
        .map(|s| s.form("refresh_token").unwrap().to_owned())
        .collect();
    assert_eq!(used.len(), 2);
    assert_ne!(
        used[0], used[1],
        "the second refresh used the token the first one was given"
    );
    assert!(
        fx.mock.secrets_seen().contains(&used[1]),
        "a token the server issued"
    );
}

#[tokio::test]
async fn a_refresh_token_the_server_keeps_is_kept() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    for _ in 0..2 {
        fx.advance(fx.access_ttl() + Duration::from_secs(1));
        fx.broker.lease(&who, &id).await.unwrap();
    }
    let used: Vec<String> = fx
        .mock
        .seen_on("/token")
        .iter()
        .filter(|s| s.form("grant_type") == Some("refresh_token"))
        .map(|s| s.form("refresh_token").unwrap().to_owned())
        .collect();
    assert_eq!(used.len(), 2);
    assert_eq!(used[0], used[1]);
    let refresh = &fx.mock.seen_on("/token")[1];
    assert_eq!(refresh.form("resource"), Some(fx.mock.mcp_url().as_str()));
}

// ------------------------------------------------------------------------------------- state

#[tokio::test]
async fn a_grant_the_server_refuses_ends_and_the_definition_is_marked() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.set_refresh_outcome(RefreshOutcome::InvalidGrant);

    let error = fx.broker.lease(&who, &id).await.unwrap_err();
    assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
    let stored = fx.servers.get(&who, &id).await.unwrap().unwrap();
    assert_eq!(stored.server().state(), UserToolServerState::NeedsReauth);
    let text = kept(&fx, &who, &id).await;
    assert!(
        text.contains("\"tokens\":null"),
        "the tokens are gone, the client stays: {text}"
    );
    assert!(text.contains("dcr-client-"));

    // marked once: a second lease changes nothing and asks nobody
    let revision = stored.revision();
    fx.broker.lease(&who, &id).await.unwrap_err();
    assert_eq!(
        fx.servers.get(&who, &id).await.unwrap().unwrap().revision(),
        revision
    );
    assert_eq!(fx.requests().refreshes, 1);
}

#[tokio::test]
async fn a_failing_server_does_not_mark_the_definition() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.set_refresh_outcome(RefreshOutcome::ServerError);
    fx.broker.lease(&who, &id).await.unwrap_err();
    let stored = fx.servers.get(&who, &id).await.unwrap().unwrap();
    assert_eq!(
        stored.server().state(),
        UserToolServerState::Added,
        "the broker writes needs_reauth only"
    );
    assert!(!kept(&fx, &who, &id).await.contains("\"tokens\":null"));
}

#[tokio::test]
async fn a_definition_edited_meanwhile_is_not_marked_by_what_was_seen_of_the_old_one() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.set_refresh_outcome(RefreshOutcome::InvalidGrant);
    fx.set_token_delay(Duration::from_millis(300));

    let broker = fx.broker.clone();
    let (who2, id2) = (who.clone(), id.clone());
    let lease = tokio::spawn(async move { broker.lease(&who2, &id2).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // the person renames the server while the broker waits for the authorization server
    let stored = fx.servers.get(&who, &id).await.unwrap().unwrap();
    let target = policy().check_url(stored.server().url()).unwrap();
    let renamed = UserToolServer::new(id.clone(), "Renamed", &target, oauth()).unwrap();
    fx.servers
        .replace(&who, &renamed, stored.revision())
        .await
        .unwrap();

    let error = lease.await.unwrap().unwrap_err();
    assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
    let now = fx.servers.get(&who, &id).await.unwrap().unwrap();
    assert_eq!(now.server().name(), "Renamed");
    assert_eq!(
        now.server().state(),
        UserToolServerState::Added,
        "the rename was not reverted"
    );
}

// ------------------------------------------------------------------------ the claim to refresh

#[tokio::test]
async fn a_refresh_claims_before_it_asks_and_leaves_no_claim_behind() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    assert!(
        kept(&fx, &who, &id)
            .await
            .contains("\"refreshing_until\":null"),
        "a grant nobody is refreshing holds no claim"
    );
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.set_token_delay(Duration::from_millis(400));

    let broker = fx.broker.clone();
    let (who2, id2) = (who.clone(), id.clone());
    let lease = tokio::spawn(async move { broker.lease(&who2, &id2).await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    // the request is with the authorization server, and the claim is in the store
    let until = fx.now().as_second() + 10 + 10;
    let text = kept(&fx, &who, &id).await;
    assert!(
        text.contains(&format!("\"refreshing_until\":{until}")),
        "the claim ends at the request timeout and a margin from now: {text}"
    );
    assert_eq!(fx.requests().refreshes, 1);

    lease.await.unwrap().expect("the refresh");
    assert!(
        kept(&fx, &who, &id)
            .await
            .contains("\"refreshing_until\":null"),
        "the tokens that answered the claim ended it"
    );
}

#[tokio::test]
async fn a_replica_that_finds_a_claim_waits_for_the_tokens_and_asks_nobody() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    // another replica is refreshing: its claim ends 20 seconds from now
    let until = fx.now().as_second() + 20;
    edit_grant(&fx, &who, &id, |g| g["refreshing_until"] = json!(until)).await;

    let broker = fx.broker.clone();
    let (who2, id2) = (who.clone(), id.clone());
    let lease = tokio::spawn(async move { broker.lease(&who2, &id2).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!lease.is_finished(), "it waits for the other replica");
    assert_eq!(fx.requests().refreshes, 0, "and asks nobody");

    // the other replica keeps its tokens
    let new_expiry = fx.now().as_second() + 3600;
    edit_grant(&fx, &who, &id, |g| {
        g["tokens"]["access"] = json!("the-others-token");
        g["tokens"]["expires_at"] = json!(new_expiry);
        g["refreshing_until"] = json!(null);
    })
    .await;
    let lease = tokio::time::timeout(Duration::from_secs(5), lease)
        .await
        .expect("it noticed")
        .unwrap()
        .expect("lease");
    assert_eq!(lease.secret().expose(), "the-others-token");
    assert_eq!(fx.requests().refreshes, 0);
}

#[tokio::test]
async fn a_claim_nobody_ends_is_taken_over_when_it_expires() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    // a replica claimed and died
    let until = fx.now().as_second() + 20;
    edit_grant(&fx, &who, &id, |g| g["refreshing_until"] = json!(until)).await;

    let broker = fx.broker.clone();
    let (who2, id2) = (who.clone(), id.clone());
    let lease = tokio::spawn(async move { broker.lease(&who2, &id2).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!lease.is_finished(), "it waits while the claim holds");
    assert_eq!(fx.requests().refreshes, 0);

    fx.advance(Duration::from_secs(21));
    tokio::time::timeout(Duration::from_secs(5), lease)
        .await
        .expect("it took over")
        .unwrap()
        .expect("lease");
    assert_eq!(
        fx.requests().refreshes,
        1,
        "one request, by the replica that took over"
    );
}

#[tokio::test]
async fn a_token_that_has_not_expired_is_lent_while_another_replica_refreshes() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    let token = access_token(&fx, &who, &id).await;
    // 10 seconds are left: inside the margin, so it would be refreshed, but another replica is
    fx.advance(fx.access_ttl() - Duration::from_secs(10));
    let until = fx.now().as_second() + 20;
    edit_grant(&fx, &who, &id, |g| g["refreshing_until"] = json!(until)).await;

    let lease = tokio::time::timeout(Duration::from_secs(2), fx.broker.lease(&who, &id))
        .await
        .expect("it did not wait")
        .expect("lease");
    assert_eq!(lease.secret().expose(), token);
    assert_eq!(lease.expires_at().as_second(), fx.now().as_second() + 10);
    assert_eq!(fx.requests().refreshes, 0);
}

#[tokio::test]
async fn a_refusal_after_another_replica_rotated_ends_nothing_and_uses_the_others_tokens() {
    // RFC 9700, 4.14.2: a replica that presents a refresh token another one has already used is
    // refused, and what it was refused is not the grant's. Whatever code the refusal has.
    for code in ["invalid_grant", "invalid_client"] {
        let fx = Fx::new().await;
        let (who, id) = authorized(&fx).await;
        fx.advance(fx.access_ttl() + Duration::from_secs(1));
        fx.set_token_delay(Duration::from_millis(400));

        let broker = fx.broker.clone();
        let (who2, id2) = (who.clone(), id.clone());
        let lease = tokio::spawn(async move { broker.lease(&who2, &id2).await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(fx.requests().refreshes, 1, "the request is out");
        // the claim ran out and another replica took over, asked, and kept its tokens
        let new_expiry = fx.now().as_second() + 3600;
        edit_grant(&fx, &who, &id, |g| {
            g["tokens"]["access"] = json!("the-winners-token");
            g["tokens"]["expires_at"] = json!(new_expiry);
            g["refreshing_until"] = json!(null);
        })
        .await;
        // and the authorization server, which has seen the refresh token twice, refuses this one
        fx.mock
            .configure(|c| c.refresh_error = Some(code.to_owned()));

        let lease = lease
            .await
            .unwrap()
            .unwrap_or_else(|e| panic!("{code}: the winner's tokens are lent, not {e:?}"));
        assert_eq!(lease.secret().expose(), "the-winners-token", "{code}");
        let stored = fx.servers.get(&who, &id).await.unwrap().unwrap();
        assert_eq!(
            stored.server().state(),
            UserToolServerState::Added,
            "{code}: nobody marked the definition"
        );
        let text = kept(&fx, &who, &id).await;
        assert!(
            !text.contains("\"tokens\":null") && text.contains("the-winners-token"),
            "{code}: the winner's tokens were not ended: {text}"
        );
    }
}

#[tokio::test]
async fn a_refusal_other_than_invalid_grant_keeps_the_tokens_marks_the_definition_and_ends_the_claim()
 {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.mock
        .configure(|c| c.refresh_error = Some("invalid_client".to_owned()));

    let error = fx.broker.lease(&who, &id).await.unwrap_err();
    assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
    let stored = fx.servers.get(&who, &id).await.unwrap().unwrap();
    assert_eq!(stored.server().state(), UserToolServerState::NeedsReauth);
    let text = kept(&fx, &who, &id).await;
    assert!(!text.contains("\"tokens\":null"), "the tokens stay: {text}");
    assert!(
        text.contains("\"refreshing_until\":null"),
        "the claim ended: {text}"
    );

    // the next lease asks again at once, with no claim to wait out
    fx.mock.configure(|c| c.refresh_error = None);
    fx.broker.lease(&who, &id).await.expect("asks again");
    assert_eq!(fx.requests().refreshes, 2);
}

#[tokio::test]
async fn a_hanging_refresh_is_one_request_and_every_waiter_hears_of_it_within_one_timeout() {
    let fx = Fx::with(
        BrokerConfig::default()
            .with_timeouts(Duration::from_millis(400), Duration::from_millis(400)),
    )
    .await;
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.set_token_delay(Duration::from_secs(5));

    let started = std::time::Instant::now();
    let results = join_all((0..6).map(|_| fx.broker.lease(&who, &id))).await;
    for result in results {
        let error = result.unwrap_err();
        assert!(
            matches!(error, CredentialError::Unavailable { .. }),
            "{error:?}"
        );
    }
    assert!(
        started.elapsed() < Duration::from_millis(2000),
        "six leases waited one timeout, not six: {:?}",
        started.elapsed()
    );
    assert_eq!(fx.requests().refreshes, 1);

    // the timeout released the claim, and the grant is intact
    fx.set_token_delay(Duration::ZERO);
    fx.broker
        .lease(&who, &id)
        .await
        .expect("asks again at once");
}

// ------------------------------------------------------------ a store that stumbles at a refresh

type FlakyStore = FailRotate<Interleaved<MemorySecretStore>>;
type FlakyBroker =
    Broker<FlakyStore, orch_ports::memory::MemoryUserToolServers, orch_ports::memory::FixedClock>;

/// A broker over `fx`'s stores whose secret store fails the `rotate` calls `arm` names (counted
/// from the first call after it: at a refresh the claim is the first).
fn over_flaky_store(fx: &Fx) -> (FlakyBroker, FlakyStore) {
    let store = FailRotate::new(Interleaved(fx.secrets.clone()));
    let broker = Broker::builder(store.clone(), fx.servers.clone(), policy())
        .clock(fx.clock.clone())
        .build()
        .unwrap();
    (broker, store)
}

#[tokio::test]
async fn a_rotate_that_fails_once_after_the_refresh_is_tried_again_and_the_new_token_is_kept() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.rotate_refresh = true);
    let (who, id) = authorized(&fx).await;
    let (flaky, store) = over_flaky_store(&fx);
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    // the claim is the first rotate, the tokens the second
    store.arm(&[2]);

    let lease = flaky.lease(&who, &id).await.expect("lease");
    assert_eq!(
        store.rotates(),
        3,
        "claim, a rotate that failed, a rotate that kept"
    );
    assert_eq!(fx.requests().refreshes, 1);
    assert_eq!(lease.secret().expose(), access_token(&fx, &who, &id).await);

    // the refresh token the server rotated to was kept: it is the one presented next
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    flaky.lease(&who, &id).await.expect("lease");
    let used: Vec<String> = fx
        .mock
        .seen_on("/token")
        .iter()
        .filter(|s| s.form("grant_type") == Some("refresh_token"))
        .map(|s| s.form("refresh_token").unwrap().to_owned())
        .collect();
    assert_eq!(used.len(), 2);
    assert_ne!(
        used[0], used[1],
        "the second refresh used the rotated token"
    );
}

// ------------------------------------------------- an authorization that ends beside other calls

type GatedBroker = Broker<
    support::Gated<Interleaved<MemorySecretStore>>,
    orch_ports::memory::MemoryUserToolServers,
    orch_ports::memory::FixedClock,
>;

/// A broker over `fx`'s stores that holds the next write of a credential at the gate.
fn over_gated_store(fx: &Fx) -> (GatedBroker, std::sync::Arc<support::Gate>) {
    let store = support::Gated::new(Interleaved(fx.secrets.clone()));
    let gate = store.gate();
    let broker = Broker::builder(store, fx.servers.clone(), policy())
        .clock(fx.clock.clone())
        .build()
        .unwrap();
    (broker, gate)
}

#[tokio::test]
async fn a_revoke_that_runs_while_an_authorization_is_kept_wins_in_this_process_and_in_another() {
    for same_process in [true, false] {
        let fx = Fx::new().await;
        let (gated, gate) = over_gated_store(&fx);
        let who = owner();
        let server = fx.add(&who, oauth()).await;
        let id = server.id().clone();
        let start = gated
            .begin_oauth(&who, &id, &fx.redirect_uri())
            .await
            .unwrap();
        let consent = fx.consent(&start.authorize_url).await.unwrap();

        gate.arm();
        let completing = {
            let (broker, who, id) = (gated.clone(), who.clone(), id.clone());
            tokio::spawn(async move {
                broker
                    .complete_oauth(
                        &who,
                        &id,
                        &start.state,
                        &consent.code,
                        consent.iss.as_deref(),
                    )
                    .await
            })
        };
        // the code is exchanged, and the tokens are about to be written
        gate.reached().await;
        let revoking = if same_process {
            let (broker, who, id) = (gated.clone(), who.clone(), id.clone());
            tokio::spawn(async move { broker.revoke(&who, &id).await })
        } else {
            // a replica: nothing is shared with it, no lock, no cache
            let (broker, who, id) = (fx.replica(), who.clone(), id.clone());
            tokio::spawn(async move { broker.revoke(&who, &id).await })
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        gate.proceed();
        revoking.await.unwrap().expect("revoke");
        let _ = completing.await.unwrap();

        let error = fx.broker.lease(&who, &id).await.unwrap_err();
        assert!(
            matches!(error, CredentialError::NeedsReauth),
            "same_process={same_process}: {error:?}"
        );
        if let Some((_, material)) = fx
            .secrets
            .get(&who, &SecretServer::credential(&id))
            .await
            .unwrap()
        {
            let text = String::from_utf8(material.expose().to_vec()).unwrap();
            assert!(
                text.contains("\"tokens\":null"),
                "same_process={same_process}: the revoked credential came back: {text}"
            );
        }
        assert_eq!(
            fx.requests().revocations,
            1,
            "same_process={same_process}: the tokens the authorization server issued were revoked"
        );
    }
}

#[tokio::test]
async fn a_flow_begun_while_another_ends_is_not_ended_by_it() {
    // at the write (the flow is looked at again after it) and during the exchange (before it)
    for at_the_write in [true, false] {
        let fx = Fx::new().await;
        let (gated, gate) = over_gated_store(&fx);
        let who = owner();
        let server = fx.add(&who, oauth()).await;
        let id = server.id().clone();
        let first = gated
            .begin_oauth(&who, &id, &fx.redirect_uri())
            .await
            .unwrap();
        let consent = fx.consent(&first.authorize_url).await.unwrap();
        if at_the_write {
            gate.arm();
        } else {
            fx.set_token_delay(Duration::from_millis(300));
        }

        let completing = {
            let (broker, who, id) = (gated.clone(), who.clone(), id.clone());
            tokio::spawn(async move {
                broker
                    .complete_oauth(
                        &who,
                        &id,
                        &first.state,
                        &consent.code,
                        consent.iss.as_deref(),
                    )
                    .await
            })
        };
        if at_the_write {
            gate.reached().await;
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // the person begins again, in another tab
        let second = fx
            .broker
            .begin_oauth(&who, &id, &fx.redirect_uri())
            .await
            .unwrap();
        if at_the_write {
            gate.proceed();
        }
        let error = completing.await.unwrap().unwrap_err();
        assert!(
            matches!(error, CredentialError::Unknown),
            "at_the_write={at_the_write}: {error:?}"
        );
        assert_eq!(fx.requests().revocations, 1, "its tokens were revoked");
        fx.set_token_delay(Duration::ZERO);
        let written = fx
            .secrets
            .get(&who, &SecretServer::credential(&id))
            .await
            .unwrap();
        if at_the_write {
            // written, and taken back
            let (_, material) = written.expect("a grant without tokens is left");
            assert!(
                String::from_utf8(material.expose().to_vec())
                    .unwrap()
                    .contains("\"tokens\":null")
            );
        } else {
            assert!(written.is_none(), "a flow that was replaced writes nothing");
        }

        // the newer flow is as it was
        let consent = fx.consent(&second.authorize_url).await.unwrap();
        fx.broker
            .complete_oauth(
                &who,
                &id,
                &second.state,
                &consent.code,
                consent.iss.as_deref(),
            )
            .await
            .unwrap_or_else(|e| panic!("at_the_write={at_the_write}: the newer flow: {e:?}"));
        fx.broker.lease(&who, &id).await.expect("lease");
    }
}

#[tokio::test]
async fn a_used_flow_keeps_nothing_of_the_flow() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    let (_, material) = fx
        .secrets
        .get(&who, &SecretServer::pending_flow(&id))
        .await
        .unwrap()
        .expect("the marker of a used flow");
    assert_eq!(material.expose(), br#"{"v":1,"d":"used"}"#);
}

// ------------------------------------------------------------------------------------ revoke

#[tokio::test]
async fn revoke_names_the_refresh_token_to_the_revocation_endpoint() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    fx.broker.revoke(&who, &id).await.unwrap();
    let request = &fx.mock.seen_on("/revoke")[0];
    assert_eq!(request.form("token_type_hint"), Some("refresh_token"));
    let token = request.form("token").unwrap();
    assert!(token.contains("rt-"), "the refresh token: {token}");
    assert!(request.form("client_id").is_some());
}

#[tokio::test]
async fn revoke_names_the_access_token_when_there_is_no_refresh_token() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.no_refresh_token = true);
    let (who, id) = authorized(&fx).await;
    fx.broker.revoke(&who, &id).await.unwrap();
    let request = &fx.mock.seen_on("/revoke")[0];
    assert_eq!(request.form("token_type_hint"), Some("access_token"));
    assert!(request.form("token").unwrap().contains("at-"));
}

#[tokio::test]
async fn revoke_succeeds_when_the_authorization_server_refuses_or_names_no_endpoint() {
    let fx = Fx::new().await;
    fx.mock.configure(|c| c.revocation_status = 503);
    let (who, id) = authorized(&fx).await;
    fx.broker.revoke(&who, &id).await.expect("best effort");
    assert_eq!(fx.mock.seen_on("/revoke").len(), 1);
    assert!(matches!(
        fx.broker.lease(&who, &id).await,
        Err(CredentialError::NeedsReauth)
    ));

    let fx = Fx::new().await;
    fx.mock.configure(|c| c.revocation = false);
    let (who, id) = authorized(&fx).await;
    fx.broker.revoke(&who, &id).await.expect("nothing to call");
    assert!(fx.mock.seen_on("/revoke").is_empty());
    assert!(matches!(
        fx.broker.lease(&who, &id).await,
        Err(CredentialError::NeedsReauth)
    ));
}

#[tokio::test]
async fn revoking_a_bearer_token_asks_nobody() {
    let fx = Fx::new().await;
    let who = owner();
    let server = fx.add(&who, UserAuth::Bearer).await;
    fx.broker
        .store(&who, server.id(), &ToolSecret::new("t"))
        .await
        .unwrap();
    fx.broker.revoke(&who, server.id()).await.unwrap();
    assert!(fx.mock.seen().is_empty());
}

#[tokio::test]
async fn revoke_finds_the_authorization_server_in_the_grant_not_in_the_definition() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    // the definition is gone; the credential is not (it was left behind)
    fx.servers.delete(&who, &id).await.unwrap();
    fx.broker.revoke(&who, &id).await.expect("revoke");
    assert_eq!(fx.mock.seen_on("/revoke").len(), 1);
    assert!(
        fx.secrets
            .get(&who, &SecretServer::credential(&id))
            .await
            .unwrap()
            .is_none()
    );
}

// --------------------------------------------------------------------------- stores that fail

#[tokio::test]
async fn a_secret_store_that_is_down_makes_the_server_unavailable_and_loses_nothing() {
    let fx = Fx::new().await;
    let who = owner();
    let bearer = fx.add(&who, UserAuth::Bearer).await;
    fx.broker
        .store(&who, bearer.id(), &ToolSecret::new("kept"))
        .await
        .unwrap();
    let (who_o, oauth_id) = authorized(&fx).await;
    let start = fx
        .broker
        .begin_oauth(&who_o, &oauth_id, &fx.redirect_uri())
        .await
        .unwrap();
    let consent = fx.consent(&start.authorize_url).await.unwrap();

    fx.secrets.set_down(true);
    let unavailable = |e: CredentialError| match e {
        CredentialError::Unavailable { ref source, .. } => {
            let cause = source.as_ref().expect("the store's error");
            assert!(
                cause.downcast_ref::<SecretError>().is_some(),
                "the cause is the store's: {cause}"
            );
        }
        other => panic!("not unavailable: {other:?}"),
    };
    unavailable(fx.broker.lease(&who, bearer.id()).await.unwrap_err());
    unavailable(
        fx.broker
            .store(&who, bearer.id(), &ToolSecret::new("new"))
            .await
            .unwrap_err(),
    );
    unavailable(fx.broker.revoke(&who, bearer.id()).await.unwrap_err());
    unavailable(fx.broker.lease(&who_o, &oauth_id).await.unwrap_err());
    unavailable(
        fx.broker
            .begin_oauth(&who_o, &oauth_id, &fx.redirect_uri())
            .await
            .unwrap_err(),
    );
    unavailable(
        fx.broker
            .complete_oauth(
                &who_o,
                &oauth_id,
                &start.state,
                &consent.code,
                consent.iss.as_deref(),
            )
            .await
            .unwrap_err(),
    );

    fx.secrets.set_down(false);
    assert_eq!(
        fx.broker
            .lease(&who, bearer.id())
            .await
            .unwrap()
            .secret()
            .expose(),
        "kept"
    );
    fx.broker
        .lease(&who_o, &oauth_id)
        .await
        .expect("the grant is kept");
    fx.broker
        .complete_oauth(
            &who_o,
            &oauth_id,
            &start.state,
            &consent.code,
            consent.iss.as_deref(),
        )
        .await
        .expect("the flow was not lost");
}

#[tokio::test]
async fn a_store_of_definitions_that_is_down_makes_every_call_unavailable_but_revoke() {
    let fx = Fx::new().await;
    let who = owner();
    let bearer = fx.add(&who, UserAuth::Bearer).await;
    fx.broker
        .store(&who, bearer.id(), &ToolSecret::new("kept"))
        .await
        .unwrap();
    fx.servers.set_down(true);
    let state = orch_ports::OAuthState::new("AAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    let results = [
        fx.broker.lease(&who, bearer.id()).await.err(),
        fx.broker
            .store(&who, bearer.id(), &ToolSecret::new("n"))
            .await
            .err(),
        fx.broker
            .begin_oauth(&who, bearer.id(), &fx.redirect_uri())
            .await
            .err(),
        fx.broker
            .complete_oauth(&who, bearer.id(), &state, &ToolSecret::new("c"), None)
            .await
            .err(),
    ];
    for error in results {
        assert!(
            matches!(error, Some(CredentialError::Unavailable { .. })),
            "{error:?}"
        );
    }
    // revoke does not read the definition: the credential is forgotten whether or not it is up
    fx.broker.revoke(&who, bearer.id()).await.expect("revoke");
    fx.servers.set_down(false);
    assert!(matches!(
        fx.broker.lease(&who, bearer.id()).await,
        Err(CredentialError::NeedsReauth)
    ));
}

// ------------------------------------------------------------------------------ cancellation

#[tokio::test]
async fn a_refresh_goes_on_when_its_caller_gives_up_and_its_tokens_are_kept() {
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    fx.set_token_delay(Duration::from_millis(300));

    let version = |fx: &Fx| {
        let (secrets, who, id) = (fx.secrets.clone(), who.clone(), id.clone());
        async move {
            secrets
                .get(&who, &SecretServer::credential(&id))
                .await
                .unwrap()
                .unwrap()
                .0
        }
    };
    let before = version(&fx).await;
    let gave_up = tokio::time::timeout(Duration::from_millis(80), fx.broker.lease(&who, &id)).await;
    assert!(gave_up.is_err(), "the caller gave up");
    // the refresh goes on without it, and keeps its tokens
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while version(&fx).await == before {
        assert!(
            std::time::Instant::now() < deadline,
            "the refresh did not finish"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(fx.requests().refreshes, 1);

    fx.set_token_delay(Duration::ZERO);
    fx.broker
        .lease(&who, &id)
        .await
        .expect("the refreshed token was kept");
    assert_eq!(fx.requests().refreshes, 1, "and not asked for again");
}

// ----------------------------------------------------------------------------- edits mid-flow

#[tokio::test]
async fn tokens_of_a_flow_for_a_url_that_was_edited_meanwhile_are_not_lent() {
    let fx = Fx::new().await;
    let who = owner();
    let (server, revision) =
        support::add_server_at(&fx.servers, &policy(), &who, oauth(), &fx.mock.mcp_url()).await;
    let start = fx
        .broker
        .begin_oauth(&who, server.id(), &fx.redirect_uri())
        .await
        .unwrap();
    let consent = fx.consent(&start.authorize_url).await.unwrap();

    let target = policy().check_url(&fx.mock.other_url()).unwrap();
    let edited = UserToolServer::new(server.id().clone(), "Edited", &target, oauth()).unwrap();
    fx.servers.replace(&who, &edited, revision).await.unwrap();

    fx.broker
        .complete_oauth(
            &who,
            server.id(),
            &start.state,
            &consent.code,
            consent.iss.as_deref(),
        )
        .await
        .expect("the flow completes for the URL it began with");
    let error = fx.broker.lease(&who, server.id()).await.unwrap_err();
    assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
}

#[tokio::test]
async fn a_grant_is_for_the_scopes_and_the_client_id_it_was_made_with() {
    let fx = Fx::new().await;
    let who = owner();
    let scopes = |names: &[&str]| names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let auth = |names: &[&str], client: Option<&str>| {
        UserAuth::oauth2(scopes(names), client.map(str::to_owned)).unwrap()
    };
    let (server, revision) = support::add_server_at(
        &fx.servers,
        &policy(),
        &who,
        auth(&["read", "write"], None),
        &fx.mock.mcp_url(),
    )
    .await;
    let id = server.id().clone();
    let start = fx
        .broker
        .begin_oauth(&who, &id, &fx.redirect_uri())
        .await
        .unwrap();
    let consent = fx.consent(&start.authorize_url).await.unwrap();
    fx.broker
        .complete_oauth(
            &who,
            &id,
            &start.state,
            &consent.code,
            consent.iss.as_deref(),
        )
        .await
        .unwrap();
    fx.broker.lease(&who, &id).await.expect("lent as made");

    let target = policy().check_url(&fx.mock.mcp_url()).unwrap();
    let edit = |names: &[&str], client: Option<&str>| {
        UserToolServer::new(id.clone(), "Edited", &target, auth(names, client)).unwrap()
    };

    // the same scopes in another order are the same scopes
    let revision = fx
        .servers
        .replace(&who, &edit(&["write", "read"], None), revision)
        .await
        .unwrap();
    fx.broker
        .lease(&who, &id)
        .await
        .expect("a reordering is no change");

    // the token has run out, so that a refresh would be tried if the grant were lent at all
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    let revision = fx
        .servers
        .replace(&who, &edit(&["read"], None), revision)
        .await
        .unwrap();
    let error = fx.broker.lease(&who, &id).await.unwrap_err();
    assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
    fx.servers
        .replace(
            &who,
            &edit(&["read", "write"], Some("another-client")),
            revision,
        )
        .await
        .unwrap();
    let error = fx.broker.lease(&who, &id).await.unwrap_err();
    assert!(matches!(error, CredentialError::NeedsReauth), "{error:?}");
    assert_eq!(
        fx.requests().refreshes,
        0,
        "a grant that is not lent is not refreshed"
    );
}

#[tokio::test]
async fn a_pending_flow_that_cannot_be_read_is_unknown() {
    let fx = Fx::new().await;
    let who = owner();
    let server = fx.add(&who, oauth()).await;
    fx.secrets
        .put(
            &who,
            &SecretServer::pending_flow(server.id()),
            &SecretMaterial::new(b"garbage".to_vec()).unwrap(),
        )
        .await
        .unwrap();
    let state = orch_ports::OAuthState::new("AAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    let error = fx
        .broker
        .complete_oauth(&who, server.id(), &state, &ToolSecret::new("c"), None)
        .await
        .unwrap_err();
    assert!(matches!(error, CredentialError::Unknown), "{error:?}");
}

#[tokio::test]
async fn the_broker_prints_nothing_of_what_it_holds() {
    let fx = Fx::new().await;
    assert_eq!(format!("{:?}", fx.broker), "Broker { .. }");
    let _ = fx.clock.now();
    let error = CredentialError::unavailable("x").with_source(std::io::Error::other("cause"));
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("cause")
    );
}
