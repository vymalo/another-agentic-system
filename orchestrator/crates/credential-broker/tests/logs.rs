//! What the broker says in its log when something goes wrong, for the operator to read: the tests
//! that need a subscriber. Every test of this file holds the subscriber alone (see
//! `Captured::install`), and the file has no test that does not.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_credential_broker::Broker;
use orch_ports::testkit::credential_broker::BrokerFixture;
use orch_ports::{CredentialBroker, CredentialError, UserAuth};
use support::{Captured, FailRotate, Fx, Interleaved, broker_over, owner, policy};

fn oauth() -> UserAuth {
    UserAuth::oauth2(Vec::new(), None).unwrap()
}

/// A server of the mock that has been authorized.
async fn authorized(fx: &Fx) -> (orch_core::UserId, orch_ports::UserToolServerId) {
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

#[tokio::test]
async fn a_store_that_does_not_recover_still_leaves_the_caller_its_valid_token_and_says_so() {
    let (log, _guard, _serial) = Captured::install().await;
    let fx = Fx::new().await;
    let (who, id) = authorized(&fx).await;
    let store = FailRotate::new(Interleaved(fx.secrets.clone()));
    let flaky = Broker::builder(store.clone(), fx.servers.clone(), policy())
        .clock(fx.clock.clone())
        .build()
        .unwrap();
    fx.advance(fx.access_ttl() + Duration::from_secs(1));
    // the claim is the first rotate; the tokens are the second, and the two tries after it
    store.arm(&[2, 3, 4]);

    let lease = flaky
        .lease(&who, &id)
        .await
        .expect("the access token is good");
    assert_eq!(store.rotates(), 4, "the claim, then three tries");
    assert!(lease.expires_at() > fx.now());
    assert_eq!(fx.requests().refreshes, 1);
    let text = log.text();
    assert!(text.contains("could not be kept"), "{text}");
    assert!(
        !text.contains(lease.secret().expose()),
        "the log shows the token"
    );
}

#[tokio::test]
async fn a_revocation_endpoint_at_the_metadata_address_is_refused_by_what_it_is_and_revoke_still_succeeds()
 {
    let (log, _guard, _serial) = Captured::install().await;
    let fx = Fx::new().await;
    fx.mock.configure(|c| {
        c.revocation_endpoint = Some("http://169.254.169.254/revoke".to_owned());
    });
    let (who, id) = authorized(&fx).await;

    // a policy whose allow list names the address, so that the list does not decide it: the class
    // of the address does (link-local is refused whatever the lists say)
    let broker = broker_over(
        &fx.secrets,
        &fx.servers,
        &fx.clock,
        policy().with_allow("169.254.169.254".parse().unwrap()),
        fx.config.clone(),
    );
    broker
        .revoke(&who, &id)
        .await
        .expect("revocation upstream is best effort");
    let text = log.text();
    assert!(
        text.contains("refused the revocation endpoint")
            && text.contains("rule=\"private-address\""),
        "the guard refused it by the class of the address: {text}"
    );
    assert!(
        text.contains(&format!("server={id}")),
        "and the log says for which server: {text}"
    );
    assert!(matches!(
        broker.lease(&who, &id).await,
        Err(CredentialError::NeedsReauth)
    ));
}
