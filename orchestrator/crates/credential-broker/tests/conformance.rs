//! The conformance suite of `CredentialBroker` (`orch-ports`) against the broker of this crate,
//! over the in-memory stores and an authorization server of the tests' own (`support`), with an
//! egress policy that names the loopback address explicitly.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

async fn make() -> Option<support::Fx> {
    Some(support::Fx::new().await)
}

orch_ports::credential_broker_conformance!(make);
