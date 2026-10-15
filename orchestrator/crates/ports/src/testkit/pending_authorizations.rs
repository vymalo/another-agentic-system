//! `PendingAuthorizations` conformance cases: what the OAuth callback relies on from any store of
//! the authorizations that have begun (ADR 0046, slice 4c), stated once and run against the in-memory
//! store and the Postgres store.
//!
//! An implementation supplies a fresh store (`async fn() -> Option<S>`; `None` skips the suite).
//! Each case is `async fn(store)`, uses owners, servers and states of its own, and gives up after 60
//! seconds. Time is the caller's: every call is given the instant it means, so the cases are about
//! the contract and never wait.
//!
//! One case needs the adapter to **break its store** (`a_broken_store_is_unavailable_and_names_nothing`):
//! the macro takes the means as a second argument, `breaker = path::to::async_fn`, an `async fn(&S)`
//! that makes every later call of that store fail as an unreachable store does (a Postgres adapter
//! closes its pool, the memory store is taken down). An adapter that cannot break its store omits
//! it, and the case is skipped with that reason printed.
//!
//! What is deliberately not asserted: how a state is kept (only that its text is not: the adapters'
//! own tests look at their rows), and the wording of any error.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{Classify, ErrorClass, UserId};
use uuid::Uuid;

use crate::{OAuthState, PendingAuthorizationError, PendingAuthorizations, UserToolServerId};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// How many callers race for one state, and how many times the race is run, each with a state of
/// its own: a race that is lost one time in a few is still found.
const TAKERS: usize = 32;
const RACE_ROUNDS: usize = 20;

/// A value no error and no `Debug` may show: it is the whole of the state, the owner and the
/// server of the case that looks.
const SENTINEL: &str = "sentinel-5b7e1d30-this-must-never-be-printed";

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A person no other case uses.
fn owner() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

/// A state no other case uses: 32 hexadecimal characters.
fn state() -> OAuthState {
    OAuthState::new(&Uuid::now_v7().simple().to_string()).expect("a state")
}

/// The server `u-<suffix>` (10 characters).
fn server(suffix: &str) -> UserToolServerId {
    UserToolServerId::parse(&format!("u-{suffix}")).expect("a server id")
}

/// An instant, `secs` seconds after a fixed one.
fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000)
        .expect("a timestamp")
        .checked_add(SignedDuration::from_secs(secs))
        .expect("a timestamp")
}

/// A state that is put comes back once, with the owner and the server it was put for, and then is
/// nothing.
pub async fn a_put_state_is_taken_once_with_its_owner_and_server<S: PendingAuthorizations>(
    store: S,
) {
    within(async {
        let (owner, server, state) = (owner(), server("aaaaaaaaaa"), state());
        store
            .put(&state, &owner, &server, at(600), at(0))
            .await
            .expect("put");
        let taken = store
            .take(&state, at(1))
            .await
            .expect("take")
            .expect("the state is pending");
        assert_eq!(taken.owner(), &owner);
        assert_eq!(taken.server(), &server);
        assert!(
            store.take(&state, at(2)).await.expect("take").is_none(),
            "nothing is returned twice"
        );
        assert!(
            store.take(&state, at(1)).await.expect("take").is_none(),
            "not even for a `now` before the first take"
        );
    })
    .await;
}

/// A state nobody put takes nothing, and neither does one that is only close to a state that is
/// pending (the same text but for one character, a prefix, another case). The pending one is
/// untouched by the attempts.
pub async fn another_state_never_matches<S: PendingAuthorizations>(store: S) {
    within(async {
        let (owner, server, pending) = (owner(), server("bbbbbbbbbb"), state());
        store
            .put(&pending, &owner, &server, at(600), at(0))
            .await
            .expect("put");
        assert!(
            store.take(&state(), at(1)).await.expect("take").is_none(),
            "a state nobody put"
        );
        let text = pending.as_str().to_owned();
        let mut near = Vec::new();
        // the last character changed
        let last = text.chars().last().expect("a character");
        let other = if last == 'a' { 'b' } else { 'a' };
        near.push(format!("{}{other}", &text[..text.len() - 1]));
        // a prefix that is still a state, and the state with something added
        near.push(text[..16].to_owned());
        near.push(format!("{text}0"));
        // another case, if the text has a letter to change
        near.push(text.to_uppercase());
        near.push(format!("x{}", &text[1..]));
        for near in near.into_iter().filter(|near| *near != text) {
            let near = OAuthState::new(&near).expect("a state");
            assert!(
                store.take(&near, at(1)).await.expect("take").is_none(),
                "a state that only looks like the pending one"
            );
        }
        let taken = store.take(&pending, at(2)).await.expect("take");
        assert!(
            taken.is_some_and(|t| t.owner() == &owner && t.server() == &server),
            "the pending state was not used up by the others"
        );
    })
    .await;
}

/// A state is valid while `now` is before `expires_at`: a take at `expires_at` and after it is
/// nothing, and the state is forgotten, not postponed.
pub async fn a_state_that_has_ended_takes_nothing<S: PendingAuthorizations>(store: S) {
    within(async {
        let owner = owner();
        // taken the instant before it ends
        let (last, last_server) = (state(), server("cccccccccc"));
        store
            .put(&last, &owner, &last_server, at(600), at(0))
            .await
            .expect("put");
        let just_before = at(600)
            .checked_sub(SignedDuration::from_nanos(1))
            .expect("a timestamp");
        assert!(
            store
                .take(&last, just_before)
                .await
                .expect("take")
                .is_some(),
            "valid until the instant it ends"
        );

        // taken at the instant it ends
        let (ends, ends_server) = (state(), server("dddddddddd"));
        store
            .put(&ends, &owner, &ends_server, at(600), at(0))
            .await
            .expect("put");
        assert!(
            store.take(&ends, at(600)).await.expect("take").is_none(),
            "ended at the instant it ends"
        );
        assert!(
            store.take(&ends, at(1)).await.expect("take").is_none(),
            "an ended state is forgotten, and a clock that goes back does not bring it back"
        );

        // taken long after
        let (late, late_server) = (state(), server("eeeeeeeeee"));
        store
            .put(&late, &owner, &late_server, at(600), at(0))
            .await
            .expect("put");
        assert!(
            store.take(&late, at(86_400)).await.expect("take").is_none(),
            "ended"
        );
    })
    .await;
}

/// Of many callers that take one state at the same time exactly one gets it, however many times the
/// race is run.
pub async fn concurrent_takes_of_one_state_have_one_winner<S: PendingAuthorizations>(store: S) {
    within(async {
        let store = Arc::new(store);
        for round in 0..RACE_ROUNDS {
            let (owner, server, state) = (owner(), server(&format!("{round:010}")), state());
            store
                .put(&state, &owner, &server, at(600), at(0))
                .await
                .expect("put");
            let takers: Vec<_> = (0..TAKERS)
                .map(|_| {
                    let (store, state) = (Arc::clone(&store), state.clone());
                    tokio::spawn(async move { store.take(&state, at(1)).await })
                })
                .collect();
            let mut winners = 0;
            for taker in takers {
                if taker.await.expect("the task").expect("take").is_some() {
                    winners += 1;
                }
            }
            assert_eq!(
                winners, 1,
                "round {round}: exactly one caller gets the state"
            );
        }
    })
    .await;
}

/// A server has one pending authorization: a later put for it forgets the earlier state, which can
/// no longer be taken. Another server of the same person, and the same server id of another person,
/// keep theirs.
pub async fn a_later_put_for_a_server_replaces_the_earlier_one<S: PendingAuthorizations>(store: S) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let (x, y) = (server("xxxxxxxxxx"), server("yyyyyyyyyy"));
        let (first, second, other_server, other_owner) = (state(), state(), state(), state());
        store
            .put(&first, &alice, &x, at(600), at(0))
            .await
            .expect("put");
        store
            .put(&other_server, &alice, &y, at(600), at(0))
            .await
            .expect("put");
        store
            .put(&other_owner, &bob, &x, at(600), at(0))
            .await
            .expect("put");
        store
            .put(&second, &alice, &x, at(600), at(1))
            .await
            .expect("put");

        assert!(
            store.take(&first, at(2)).await.expect("take").is_none(),
            "the state the server had before"
        );
        let taken = store
            .take(&second, at(2))
            .await
            .expect("take")
            .expect("pending");
        assert_eq!((taken.owner(), taken.server()), (&alice, &x));
        let taken = store
            .take(&other_server, at(2))
            .await
            .expect("take")
            .expect("pending");
        assert_eq!((taken.owner(), taken.server()), (&alice, &y));
        let taken = store
            .take(&other_owner, at(2))
            .await
            .expect("take")
            .expect("pending");
        assert_eq!((taken.owner(), taken.server()), (&bob, &x));
    })
    .await;
}

/// A state belongs to the one it was put for: two people who hold the same server id (the id is
/// the person's own key) each get their own back, and never the other's.
pub async fn owners_are_kept_apart<S: PendingAuthorizations>(store: S) {
    within(async {
        let (alice, bob, x) = (owner(), owner(), server("zzzzzzzzzz"));
        let (for_alice, for_bob) = (state(), state());
        store
            .put(&for_alice, &alice, &x, at(600), at(0))
            .await
            .expect("put");
        store
            .put(&for_bob, &bob, &x, at(600), at(0))
            .await
            .expect("put");
        let taken = store
            .take(&for_bob, at(1))
            .await
            .expect("take")
            .expect("pending");
        assert_eq!(taken.owner(), &bob);
        let taken = store
            .take(&for_alice, at(1))
            .await
            .expect("take")
            .expect("pending");
        assert_eq!(taken.owner(), &alice);
    })
    .await;
}

/// A state that is put twice belongs to the later put (a state is made once, from random bytes, so
/// this is not a case a caller meets; an adapter must still not fail, or keep both).
pub async fn the_same_state_put_again_belongs_to_the_later_put<S: PendingAuthorizations>(store: S) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let (x, y) = (server("pppppppppp"), server("qqqqqqqqqq"));
        let state = state();
        store
            .put(&state, &alice, &x, at(600), at(0))
            .await
            .expect("put");
        store
            .put(&state, &bob, &y, at(600), at(1))
            .await
            .expect("put");
        let taken = store
            .take(&state, at(2))
            .await
            .expect("take")
            .expect("pending");
        assert_eq!((taken.owner(), taken.server()), (&bob, &y));
        assert!(
            store.take(&state, at(2)).await.expect("take").is_none(),
            "and once"
        );
    })
    .await;
}

/// A put forgets every state that has ended by the `now` it is given, so the states of flows nobody
/// came back to do not accumulate. (Seen through the port: the ended state is nothing even to a
/// caller whose clock says it is still valid.)
pub async fn a_put_forgets_the_states_that_have_ended<S: PendingAuthorizations>(store: S) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let (x, y) = (server("kkkkkkkkkk"), server("llllllllll"));
        let (abandoned, live, fresh) = (state(), state(), state());
        store
            .put(&abandoned, &alice, &x, at(600), at(0))
            .await
            .expect("put");
        store
            .put(&live, &bob, &y, at(5_000), at(0))
            .await
            .expect("put");
        // ten minutes later someone else begins a flow: the abandoned one ended at 600
        store
            .put(&fresh, &alice, &y, at(1_300), at(700))
            .await
            .expect("put");
        assert!(
            store
                .take(&abandoned, at(100))
                .await
                .expect("take")
                .is_none(),
            "forgotten by the put that came after its end"
        );
        assert!(
            store.take(&live, at(701)).await.expect("take").is_some(),
            "a state that has not ended is kept"
        );
        assert!(store.take(&fresh, at(701)).await.expect("take").is_some());
    })
    .await;
}

/// A store that cannot be reached is `Unavailable` with the class `Transient`, and nothing it says
/// (`Display`, `Debug`, the chain of sources) holds the state, the owner or the server.
pub async fn a_broken_store_is_unavailable_and_names_nothing<S: PendingAuthorizations>(
    store: S,
    break_store: impl AsyncFnOnce(&S),
) {
    within(async {
        let state = OAuthState::new(SENTINEL).expect("a state");
        let owner = UserId::new(&format!("{SENTINEL}@conformance.test"));
        let server = server("ssssssssss");
        store
            .put(&state, &owner, &server, at(600), at(0))
            .await
            .expect("put, while the store is whole");

        break_store(&store).await;

        let errors: [PendingAuthorizationError; 2] = [
            store
                .put(&state, &owner, &server, at(600), at(0))
                .await
                .expect_err("a broken store cannot put"),
            store
                .take(&state, at(1))
                .await
                .expect_err("a broken store cannot take"),
        ];
        for error in &errors {
            assert!(
                matches!(error, PendingAuthorizationError::Unavailable { .. }),
                "{error:?}"
            );
            assert_eq!(error.class(), ErrorClass::Transient);
            let said = everything_said_by(error);
            for hidden in [SENTINEL, server.as_str()] {
                assert!(!said.contains(hidden), "{hidden} is in: {said}");
            }
        }
    })
    .await;
}

/// The `Display`, the `Debug` and the same two of every source of `error`, one after the other.
fn everything_said_by(error: &(dyn std::error::Error + 'static)) -> String {
    let mut said = String::new();
    let mut at: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = at {
        said.push_str(&format!("{e} | {e:?} | "));
        at = e.source();
    }
    said
}
