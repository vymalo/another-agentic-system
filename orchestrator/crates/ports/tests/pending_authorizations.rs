//! `PendingAuthorizations` (ADR 0046, slice 4c): the conformance suite against the in-memory store,
//! and what the suite does not say: the refusing store, that a state is not kept as text, that a
//! server's pending state is one row however often it is replaced, and that nothing prints an owner.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use jiff::{SignedDuration, Timestamp};
use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::memory::MemoryPendingAuthorizations;
use orch_ports::{
    NoPendingAuthorizations, OAuthState, PendingAuthorization, PendingAuthorizations, StateDigest,
    UserToolServerId,
};

async fn make() -> Option<MemoryPendingAuthorizations> {
    Some(MemoryPendingAuthorizations::new())
}

async fn break_store(store: &MemoryPendingAuthorizations) {
    store.set_down(true);
}

orch_ports::pending_authorizations_conformance!(make, breaker = break_store);

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000)
        .unwrap()
        .checked_add(SignedDuration::from_secs(secs))
        .unwrap()
}

fn state(text: &str) -> OAuthState {
    OAuthState::new(text).unwrap()
}

fn server(suffix: &str) -> UserToolServerId {
    UserToolServerId::parse(&format!("u-{suffix}")).unwrap()
}

#[tokio::test]
async fn a_server_that_authorizes_again_and_again_is_one_row() {
    let store = MemoryPendingAuthorizations::new();
    let (owner, x) = (UserId::new("alice@example.test"), server("aaaaaaaaaa"));
    for n in 0..50 {
        store
            .put(
                &state(&format!("state-number-{n:04}-abcdef")),
                &owner,
                &x,
                at(600 + n),
                at(n),
            )
            .await
            .unwrap();
    }
    assert_eq!(store.len(), 1, "the later put replaces the earlier");
}

#[tokio::test]
async fn abandoned_flows_of_other_servers_are_swept_by_the_next_put() {
    let store = MemoryPendingAuthorizations::new();
    let owner = UserId::new("alice@example.test");
    for n in 0..8 {
        store
            .put(
                &state(&format!("abandoned-state-{n:04}-abc")),
                &owner,
                &server(&format!("{n:010}")),
                at(600),
                at(0),
            )
            .await
            .unwrap();
    }
    assert_eq!(store.len(), 8);
    store
        .put(
            &state("fresh-state-0000-abcdef"),
            &owner,
            &server("9999999999"),
            at(1_300),
            at(700),
        )
        .await
        .unwrap();
    assert_eq!(store.len(), 1, "the eight that ended are gone");
}

#[tokio::test]
async fn the_store_is_keyed_by_the_digest_and_a_taken_state_leaves_nothing() {
    let store = MemoryPendingAuthorizations::new();
    let planted = state("SENTINEL-state-0123456789abcdef");
    store
        .put(
            &planted,
            &UserId::new("a@example.test"),
            &server("bbbbbbbbbb"),
            at(600),
            at(0),
        )
        .await
        .unwrap();
    // nothing the store prints holds the text of the state
    assert!(!format!("{store:?}").contains("SENTINEL"), "{store:?}");
    assert!(store.take(&planted, at(1)).await.unwrap().is_some());
    assert!(
        store.is_empty(),
        "a state that was taken is gone, not marked"
    );
}

#[test]
fn the_digest_is_the_sha256_of_the_state_and_prints_nothing_of_it() {
    use sha2::{Digest, Sha256};

    let digest = StateDigest::of(&state("0123456789abcdef"));
    let expected: [u8; 32] = Sha256::digest(b"0123456789abcdef").into();
    assert_eq!(digest.as_bytes(), &expected);
    assert_ne!(
        StateDigest::of(&state("0123456789abcdeg")),
        digest,
        "one character changes it"
    );
    let shown = format!("{digest:?}");
    assert!(!shown.contains("0123456789abcdef"), "{shown}");
    assert!(!shown.contains(&format!("{:02x}", expected[0])), "{shown}");
}

#[test]
fn what_a_pending_authorization_prints_is_its_server_and_not_its_owner() {
    let pending = PendingAuthorization::new(
        UserId::new("alice-secret@example.test"),
        server("cccccccccc"),
    );
    let shown = format!("{pending:?}");
    assert!(shown.contains("u-cccccccccc"), "{shown}");
    assert!(!shown.contains("alice"), "{shown}");
    let (owner, id) = pending.into_parts();
    assert_eq!(
        (owner.as_str(), id.as_str()),
        ("alice-secret@example.test", "u-cccccccccc")
    );
}

#[tokio::test]
async fn the_refusing_store_refuses_every_call_and_names_nothing() {
    let refusing = NoPendingAuthorizations;
    let owner = UserId::new("alice-sentinel@example.test");
    let planted = state("SENTINEL-state-0123456789abcdef");
    let errors = [
        refusing
            .put(&planted, &owner, &server("dddddddddd"), at(600), at(0))
            .await
            .unwrap_err(),
        refusing.take(&planted, at(1)).await.unwrap_err(),
    ];
    for error in errors {
        assert_eq!(error.class(), ErrorClass::Transient);
        let shown = format!("{error} {error:?}");
        for hidden in ["SENTINEL", "alice", "dddddddddd"] {
            assert!(!shown.contains(hidden), "{hidden} is in: {shown}");
        }
    }
}
