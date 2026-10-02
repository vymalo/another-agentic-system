//! Runs the `Authenticator` conformance testkit against the in-memory authenticator, and pins
//! `RefuseAll` and the composition of two authenticators.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::UserId;
use orch_ports::memory::MemoryAuth;
use orch_ports::testkit::authenticator::{AuthFixture, Presented};
use orch_ports::{Principal, Role};

struct Memory(MemoryAuth);

impl AuthFixture for Memory {
    type Auth = MemoryAuth;

    fn auth(&self) -> &MemoryAuth {
        &self.0
    }

    fn valid(&self) -> (Presented, UserId) {
        (
            Presented::bearer("token-alice"),
            UserId::new("alice@example.com"),
        )
    }

    fn refused(&self) -> Vec<Presented> {
        vec![
            Presented::bearer("token-mallory"),
            Presented::bearer(""),
            // The identity header is not a credential of a bearer authenticator, but when a
            // bearer comes with it, the bearer decides.
            Presented {
                bearer: Some("token-forged".to_owned()),
                identity_header: Some("alice@example.com".to_owned()),
            },
        ]
    }

    fn set_down(&self, down: bool) -> bool {
        self.0.set_down(down);
        true
    }
}

async fn make() -> Option<Memory> {
    let auth = MemoryAuth::new();
    let mut alice = Principal::of(UserId::new("alice@example.com"));
    alice.roles.insert(Role::new("user"));
    auth.allow("token-alice", alice);
    Some(Memory(auth))
}

orch_ports::authenticator_conformance!(make);
