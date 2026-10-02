//! The bearer-token conformance testkit against the JWT authenticator, over real HTTP, against a
//! token issuer on a local port: RSA, EC and Ed25519 keys made in the test process, published
//! by discovery, rotated and taken down as the cases say.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::future::Future;
use std::time::Duration;

use orch_auth_jwt::testkit::TestIdp;
use orch_auth_jwt::{JwtAuth, JwtConfig};
use orch_ports::testkit::bearer::{Signing, TokenFixture};
use serde_json::{Map, Value};

const AUDIENCE: &str = "orchestrator-web";
const USER_CLAIM: &str = "preferred_username";
const ROLES_CLAIM: &str = "realm_access.roles";

/// Short intervals: a case never waits on the real ones.
const INTERVAL: Duration = Duration::from_millis(300);

struct Fixture {
    idp: TestIdp,
    auth: JwtAuth,
}

impl TokenFixture for Fixture {
    type Auth = JwtAuth;

    fn auth(&self) -> &JwtAuth {
        &self.auth
    }

    fn issuer(&self) -> String {
        self.idp.issuer()
    }

    fn audience(&self) -> String {
        AUDIENCE.to_owned()
    }

    fn user_claim(&self) -> String {
        USER_CLAIM.to_owned()
    }

    fn roles_claim(&self) -> String {
        ROLES_CLAIM.to_owned()
    }

    fn mint(&self, claims: &Map<String, Value>, signing: Signing) -> String {
        self.idp.mint(claims, signing)
    }

    fn set_jwks_down(&self, down: bool) {
        self.idp.set_jwks_down(down);
    }

    fn jwks_fetches(&self) -> usize {
        self.idp.jwks_fetches()
    }

    fn publish_unpublished_keys(&self) {
        self.idp.publish_unpublished_keys();
    }

    fn wait_out_the_refetch_interval(&self) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(INTERVAL + Duration::from_millis(100))
    }
}

async fn make() -> Option<Fixture> {
    let idp = TestIdp::start().await;
    let mut cfg = JwtConfig::new(idp.issuer(), [AUDIENCE])
        .with_user_claim(USER_CLAIM)
        .with_roles_claim(ROLES_CLAIM)
        .without_system_proxy();
    cfg.kid_refetch_interval = INTERVAL;
    cfg.retry_interval = INTERVAL;
    let auth = JwtAuth::new(cfg).unwrap();
    Some(Fixture { idp, auth })
}

orch_ports::bearer_token_conformance!(make);
