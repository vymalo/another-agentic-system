//! `UserToolServerStore` conformance cases: what the API, the broker and the relay rely on from any
//! store of a person's MCP server definitions (ADR 0046), stated once and run against the in-memory
//! store and the Postgres store.
//!
//! An implementation supplies a fresh store (`async fn() -> Option<S>`; `None` skips the suite).
//! Each case is `async fn(store)`, uses owners of its own (so a store shared by the cases of one
//! run is never confused) and gives up after 60 seconds.
//!
//! What is deliberately not asserted: how a definition is kept, what a revision is beyond "a new one
//! for every write, never given twice for a key, and increasing within a key", and the wording of
//! any error.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use orch_core::{Classify, ErrorClass, UserId};
use uuid::Uuid;

use crate::{
    CredentialRef, EgressPolicy, MAX_USER_TOOL_SERVERS, ServerRevision, UserAuth, UserToolServer,
    UserToolServerId, UserToolServerState, UserToolServerStore, UserToolServerStoreError,
};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// How many times the cases that race writers are run, each time with an owner of its own: a race
/// that is lost one time in a few is still found.
const RACE_ROUNDS: usize = 20;

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A person no other case uses.
fn owner() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

/// A definition at `url` (checked by the guard, as a caller must) with the id `u-<suffix>` (10
/// characters), the name `name` and the given kind.
fn server_at(suffix: &str, name: &str, url: &str, auth: UserAuth) -> UserToolServer {
    let id = UserToolServerId::parse(&format!("u-{suffix}")).expect("a valid id");
    let target = EgressPolicy::new()
        .check_url(url)
        .expect("a URL the guard accepts");
    UserToolServer::new(id, name, &target, auth).expect("a valid definition")
}

fn server_with(suffix: &str, auth: UserAuth) -> UserToolServer {
    server_at(
        suffix,
        &format!("Server {suffix}"),
        "https://mcp.example.com/mcp",
        auth,
    )
}

fn server(suffix: &str) -> UserToolServer {
    server_with(suffix, UserAuth::None)
}

/// The id `u-` and ten digits of `n`.
fn id_of(n: usize) -> String {
    format!("{n:010}")
}

/// The definitions of an owner, without their revisions.
async fn servers_of<S: UserToolServerStore>(store: &S, owner: &UserId) -> Vec<UserToolServer> {
    store
        .list(owner)
        .await
        .expect("list")
        .into_iter()
        .map(crate::StoredServer::into_server)
        .collect()
}

/// A definition of every kind comes back whole with the revision `insert` gave: id, name, URL, kind
/// with its header, scopes and client id, state and credential reference.
pub async fn insert_then_get_round_trips<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let kinds = [
            UserAuth::None,
            UserAuth::Bearer,
            UserAuth::api_key("X-Api-Key").expect("a header"),
            UserAuth::oauth2(
                vec!["repo".into(), "read:org".into()],
                Some("client-1".into()),
            )
            .expect("oauth2"),
            UserAuth::oauth2(Vec::new(), None).expect("oauth2 with nothing"),
        ];
        for (n, kind) in kinds.into_iter().enumerate() {
            let definition = server_with(&id_of(n), kind)
                .with_state(UserToolServerState::Connected)
                .with_credential(CredentialRef::new(&format!("cred-{n}")).expect("a ref"));
            let revision = store.insert(&owner, &definition).await.expect("insert");
            let got = store
                .get(&owner, definition.id())
                .await
                .expect("get")
                .expect("the definition is kept");
            assert_eq!(got.server(), &definition);
            assert_eq!(got.revision(), revision);
        }
    })
    .await;
}

/// A definition that is not there is `None`, not an error.
pub async fn a_missing_server_is_none<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let id = UserToolServerId::parse("u-0000000001").unwrap();
        assert!(store.get(&owner, &id).await.expect("get").is_none());
        assert!(store.list(&owner).await.expect("list").is_empty());
    })
    .await;
}

/// A new definition has no credential and has been added, nothing more.
pub async fn a_definition_holds_no_secret_and_starts_added<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let definition = server_with("00000000aa", UserAuth::Bearer);
        store.insert(&owner, &definition).await.expect("insert");
        let got = store.get(&owner, definition.id()).await.unwrap().unwrap();
        assert_eq!(got.server().state(), UserToolServerState::Added);
        assert!(got.server().credential().is_none());
    })
    .await;
}

/// `list` returns the owner's definitions with their revisions, ordered by id, and nobody else's.
pub async fn list_returns_the_owners_servers_ordered_by_id<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        // inserted out of order, so an order of insertion is not mistaken for the order of ids
        let mut revisions = Vec::new();
        for suffix in ["00000000cc", "00000000aa", "00000000bb"] {
            revisions.push((
                format!("u-{suffix}"),
                store.insert(&owner, &server(suffix)).await.expect("insert"),
            ));
        }
        let listed = store.list(&owner).await.expect("list");
        let ids: Vec<String> = listed.iter().map(|s| s.server().id().to_string()).collect();
        assert_eq!(ids, ["u-00000000aa", "u-00000000bb", "u-00000000cc"]);
        for stored in &listed {
            let (_, inserted) = revisions
                .iter()
                .find(|(id, _)| id == stored.server().id().as_str())
                .expect("one that was inserted");
            assert_eq!(&stored.revision(), inserted);
        }
    })
    .await;
}

/// Nobody reads, lists, changes or removes another person's definition: for the other person it is
/// not there, even with the right revision in hand.
pub async fn another_owner_sees_nothing<S: UserToolServerStore>(store: S) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let definition =
            server_with("00000000aa", UserAuth::Bearer).with_state(UserToolServerState::Connected);
        let revision = store.insert(&alice, &definition).await.expect("insert");

        assert!(store.get(&bob, definition.id()).await.unwrap().is_none());
        assert!(store.list(&bob).await.unwrap().is_empty());
        let error = store
            .set_state(
                &bob,
                definition.id(),
                UserToolServerState::Revoked,
                revision,
            )
            .await
            .expect_err("bob cannot change alice's server");
        assert_eq!(error.class(), ErrorClass::NotFound, "{error}");
        let error = store
            .replace(&bob, &definition, revision)
            .await
            .expect_err("bob cannot replace alice's server");
        assert_eq!(error.class(), ErrorClass::NotFound, "{error}");
        assert!(store.get(&bob, definition.id()).await.unwrap().is_none());
        store
            .delete(&bob, definition.id())
            .await
            .expect("delete is idempotent");

        // and none of it touched alice's
        let kept = store.get(&alice, definition.id()).await.unwrap().unwrap();
        assert_eq!(kept.server(), &definition);
        assert_eq!(kept.revision(), revision);
        assert_eq!(store.list(&alice).await.unwrap().len(), 1);
    })
    .await;
}

/// An id is an owner's: two people may each hold `u-…` with the same text and never meet.
pub async fn the_same_id_can_belong_to_two_owners<S: UserToolServerStore>(store: S) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let a = server("00000000aa").with_state(UserToolServerState::Connected);
        let b = server_with("00000000aa", UserAuth::Bearer);
        store.insert(&alice, &a).await.unwrap();
        store.insert(&bob, &b).await.unwrap();
        assert_eq!(
            store.get(&alice, a.id()).await.unwrap().unwrap().server(),
            &a
        );
        assert_eq!(store.get(&bob, b.id()).await.unwrap().unwrap().server(), &b);
        store.delete(&alice, a.id()).await.unwrap();
        assert!(store.get(&alice, a.id()).await.unwrap().is_none());
        assert_eq!(store.get(&bob, b.id()).await.unwrap().unwrap().server(), &b);
    })
    .await;
}

/// `insert` of an id the owner already holds is `Exists` (class `Rejected`) and changes nothing,
/// not even at the limit, where it is not "too many".
pub async fn insert_of_an_id_that_exists_is_refused_and_changes_nothing<S: UserToolServerStore>(
    store: S,
) {
    within(async {
        let owner = owner();
        let first =
            server_with("00000000aa", UserAuth::Bearer).with_state(UserToolServerState::Connected);
        let revision = store.insert(&owner, &first).await.unwrap();

        let again = server_at(
            "00000000aa",
            "Other",
            "https://other.example.com/",
            UserAuth::None,
        );
        let error = store
            .insert(&owner, &again)
            .await
            .expect_err("the id is taken");
        assert!(
            matches!(error, UserToolServerStoreError::Exists),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Rejected);
        let kept = store.get(&owner, first.id()).await.unwrap().unwrap();
        assert_eq!((kept.server(), kept.revision()), (&first, revision));

        // at the limit it is still "exists", and writes nothing
        for n in 1..MAX_USER_TOOL_SERVERS {
            store.insert(&owner, &server(&id_of(n))).await.unwrap();
        }
        assert_eq!(
            servers_of(&store, &owner).await.len(),
            MAX_USER_TOOL_SERVERS
        );
        let error = store
            .insert(&owner, &again)
            .await
            .expect_err("the id is taken");
        assert!(
            matches!(error, UserToolServerStoreError::Exists),
            "{error:?}"
        );
        assert_eq!(
            servers_of(&store, &owner).await.len(),
            MAX_USER_TOOL_SERVERS
        );
    })
    .await;
}

/// `replace` with the revision that is stored replaces the definition whole (the edit): nothing of
/// the old one is kept, the number of definitions does not change, and the revision is a new one.
pub async fn replace_with_the_current_revision_replaces_whole<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let first = server_with("00000000aa", UserAuth::api_key("X-Key").unwrap())
            .with_state(UserToolServerState::Connected)
            .with_credential(CredentialRef::new("cred-old").unwrap());
        let r1 = store.insert(&owner, &first).await.unwrap();

        let second = server_at(
            "00000000aa",
            "Renamed",
            "https://other.example.com/mcp",
            UserAuth::Bearer,
        );
        let r2 = store.replace(&owner, &second, r1).await.expect("replace");
        assert_ne!(r1, r2);
        assert!(r2 > r1, "{r2:?} should be newer than {r1:?}");

        let got = store.get(&owner, first.id()).await.unwrap().unwrap();
        assert_eq!(got.server(), &second);
        assert_eq!(got.revision(), r2);
        assert_eq!(got.server().name(), "Renamed");
        assert!(got.server().credential().is_none());
        assert_eq!(got.server().state(), UserToolServerState::Added);
        assert_eq!(servers_of(&store, &owner).await.len(), 1);

        // and can be done again from the new revision, which is how a credential reference is set
        let third = second
            .clone()
            .with_credential(CredentialRef::new("cred-new").unwrap())
            .with_state(UserToolServerState::Connected);
        let r3 = store
            .replace(&owner, &third, r2)
            .await
            .expect("replace again");
        assert!(r3 > r2);
        let got = store.get(&owner, first.id()).await.unwrap().unwrap();
        assert_eq!((got.server(), got.revision()), (&third, r3));
    })
    .await;
}

/// `replace` with a revision that is not the stored one is a `Conflict` (class `Conflict`) and
/// changes nothing: after a `replace`, and after a `set_state`. This is the lost update an upsert
/// would make: an edit that read the definition before the broker marked it.
pub async fn a_stale_replace_is_a_conflict_and_changes_nothing<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let first = server_with("00000000aa", UserAuth::Bearer);
        let r1 = store.insert(&owner, &first).await.unwrap();

        // the broker marks it connected while an edit holds r1
        let r2 = store
            .set_state(&owner, first.id(), UserToolServerState::Connected, r1)
            .await
            .unwrap();
        let edit = server_at(
            "00000000aa",
            "Edited",
            "https://mcp.example.com/mcp",
            UserAuth::Bearer,
        );
        let error = store
            .replace(&owner, &edit, r1)
            .await
            .expect_err("a revision a set_state replaced");
        assert!(
            matches!(error, UserToolServerStoreError::Conflict),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Conflict);
        let kept = store.get(&owner, first.id()).await.unwrap().unwrap();
        assert_eq!(kept.revision(), r2);
        assert_eq!(kept.server().state(), UserToolServerState::Connected);
        assert_eq!(
            kept.server().name(),
            first.name(),
            "the edit was not applied"
        );

        // and after a replace
        let r3 = store.replace(&owner, &edit, r2).await.unwrap();
        let error = store
            .replace(&owner, &first, r2)
            .await
            .expect_err("a revision a replace replaced");
        assert!(
            matches!(error, UserToolServerStoreError::Conflict),
            "{error:?}"
        );
        let kept = store.get(&owner, first.id()).await.unwrap().unwrap();
        assert_eq!((kept.server(), kept.revision()), (&edit, r3));
    })
    .await;
}

/// `set_state` with a stale revision is a `Conflict` and changes nothing, so a state computed about
/// a definition that was edited since is not written onto the edited one.
pub async fn a_stale_set_state_is_a_conflict_and_changes_nothing<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let first = server_with("00000000aa", UserAuth::Bearer);
        let r1 = store.insert(&owner, &first).await.unwrap();
        let edit = server_at(
            "00000000aa",
            "Edited",
            "https://moved.example.com/mcp",
            UserAuth::Bearer,
        );
        let r2 = store.replace(&owner, &edit, r1).await.unwrap();

        let error = store
            .set_state(&owner, first.id(), UserToolServerState::NeedsReauth, r1)
            .await
            .expect_err("the broker's read is from before the edit");
        assert!(
            matches!(error, UserToolServerStoreError::Conflict),
            "{error:?}"
        );
        let kept = store.get(&owner, first.id()).await.unwrap().unwrap();
        assert_eq!((kept.server(), kept.revision()), (&edit, r2));
    })
    .await;
}

/// A `replace` or a `set_state` of a definition that was deleted since it was read is `NotFound`
/// and writes nothing: the server stays deleted.
pub async fn a_write_after_a_delete_is_not_found_and_brings_nothing_back<S: UserToolServerStore>(
    store: S,
) {
    within(async {
        let owner = owner();
        let definition = server_with("00000000aa", UserAuth::Bearer);
        let revision = store.insert(&owner, &definition).await.unwrap();
        store.delete(&owner, definition.id()).await.unwrap();

        let error = store
            .replace(&owner, &definition, revision)
            .await
            .expect_err("deleted since it was read");
        assert!(
            matches!(error, UserToolServerStoreError::NotFound),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::NotFound);
        let error = store
            .set_state(
                &owner,
                definition.id(),
                UserToolServerState::Connected,
                revision,
            )
            .await
            .expect_err("deleted since it was read");
        assert!(
            matches!(error, UserToolServerStoreError::NotFound),
            "{error:?}"
        );

        assert!(store.get(&owner, definition.id()).await.unwrap().is_none());
        assert!(store.list(&owner).await.unwrap().is_empty());

        // and one that never was
        let never = server("00000000zz");
        let error = store
            .replace(&owner, &never, ServerRevision::new(1))
            .await
            .expect_err("nothing to replace");
        assert!(
            matches!(error, UserToolServerStoreError::NotFound),
            "{error:?}"
        );
        assert!(store.list(&owner).await.unwrap().is_empty());
    })
    .await;
}

/// Every write gives the definition a new revision, and a revision is not given twice for a key,
/// even when the definition is deleted and added again: a caller that read before the delete can not
/// write onto the new one.
pub async fn revisions_are_new_with_every_write_and_never_given_twice<S: UserToolServerStore>(
    store: S,
) {
    within(async {
        let owner = owner();
        let definition = server_with("00000000aa", UserAuth::Bearer);
        let r1 = store.insert(&owner, &definition).await.unwrap();
        let r2 = store
            .set_state(&owner, definition.id(), UserToolServerState::Connected, r1)
            .await
            .unwrap();
        // a write that changes nothing is still a write
        let r3 = store
            .set_state(&owner, definition.id(), UserToolServerState::Connected, r2)
            .await
            .unwrap();
        let r4 = store.replace(&owner, &definition, r3).await.unwrap();
        assert!(r1 < r2 && r2 < r3 && r3 < r4, "{r1:?} {r2:?} {r3:?} {r4:?}");

        store.delete(&owner, definition.id()).await.unwrap();
        let again = store.insert(&owner, &definition).await.unwrap();
        for old in [r1, r2, r3, r4] {
            assert_ne!(
                again, old,
                "a revision read before the delete matches again"
            );
        }
        let error = store
            .replace(&owner, &definition, r4)
            .await
            .expect_err("the revision read before the delete");
        assert!(
            matches!(error, UserToolServerStoreError::Conflict),
            "{error:?}"
        );
        let error = store
            .set_state(&owner, definition.id(), UserToolServerState::Revoked, r4)
            .await
            .expect_err("the revision read before the delete");
        assert!(
            matches!(error, UserToolServerStoreError::Conflict),
            "{error:?}"
        );
        let kept = store.get(&owner, definition.id()).await.unwrap().unwrap();
        assert_eq!((kept.server(), kept.revision()), (&definition, again));
    })
    .await;
}

/// A person holds at most [`MAX_USER_TOOL_SERVERS`]: the next one is refused (class `Rejected`) and
/// writes nothing; replacing one at the limit is not adding; removing one makes room; another
/// person's count is their own.
pub async fn at_most_eight_servers_per_owner<S: UserToolServerStore>(store: S) {
    within(async {
        assert_eq!(MAX_USER_TOOL_SERVERS, 8, "the limit is the ADR's");
        let (owner, other) = (owner(), owner());
        let mut revisions = Vec::new();
        for n in 0..MAX_USER_TOOL_SERVERS {
            revisions.push(
                store
                    .insert(&owner, &server(&id_of(n)))
                    .await
                    .unwrap_or_else(|e| panic!("server {n} is within the limit: {e}")),
            );
        }
        let ninth = server(&id_of(MAX_USER_TOOL_SERVERS));
        let error = store
            .insert(&owner, &ninth)
            .await
            .expect_err("a ninth server is refused");
        assert!(
            matches!(error, UserToolServerStoreError::TooMany { limit: 8 }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Rejected);
        assert!(store.get(&owner, ninth.id()).await.unwrap().is_none());
        assert_eq!(
            servers_of(&store, &owner).await.len(),
            MAX_USER_TOOL_SERVERS
        );

        // an edit of one of the eight is not a ninth
        let edit = server_with(&id_of(3), UserAuth::Bearer);
        store
            .replace(&owner, &edit, revisions[3])
            .await
            .expect("an edit at the limit");
        assert_eq!(
            servers_of(&store, &owner).await.len(),
            MAX_USER_TOOL_SERVERS
        );

        // another person is unaffected, and the first can add again after a removal
        store
            .insert(&other, &ninth)
            .await
            .expect("another owner's count is their own");
        store.delete(&owner, edit.id()).await.unwrap();
        store
            .insert(&owner, &ninth)
            .await
            .expect("room after a removal");
        assert_eq!(
            servers_of(&store, &owner).await.len(),
            MAX_USER_TOOL_SERVERS
        );
    })
    .await;
}

/// Writers that race for the last places never pass the limit: of sixteen concurrent adds, eight
/// succeed and eight are refused for being too many. Run twenty times, each with an owner of its
/// own, because a count that is read and then written passes the limit only when the writers
/// interleave, which is not every time.
pub async fn concurrent_inserts_never_pass_the_limit<S: UserToolServerStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for round in 0..RACE_ROUNDS {
            let owner = owner();
            let tasks: Vec<_> = (0..2 * MAX_USER_TOOL_SERVERS)
                .map(|n| {
                    let store = Arc::clone(&store);
                    let owner = owner.clone();
                    tokio::spawn(async move { store.insert(&owner, &server(&id_of(n))).await })
                })
                .collect();
            let (mut ok, mut refused) = (0, 0);
            for task in tasks {
                match task.await.expect("the task ran") {
                    Ok(_) => ok += 1,
                    Err(UserToolServerStoreError::TooMany { .. }) => refused += 1,
                    Err(other) => panic!("neither added nor refused for being too many: {other}"),
                }
            }
            assert_eq!(
                (ok, refused),
                (MAX_USER_TOOL_SERVERS, MAX_USER_TOOL_SERVERS),
                "round {round}"
            );
            assert_eq!(
                servers_of(store.as_ref(), &owner).await.len(),
                MAX_USER_TOOL_SERVERS,
                "round {round}"
            );
        }
    })
    .await;
}

/// Writers that race to add one id have one winner: the others get `Exists`, and what is kept is the
/// winner's, whole.
pub async fn concurrent_inserts_of_one_id_have_one_winner<S: UserToolServerStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for round in 0..RACE_ROUNDS {
            let owner = owner();
            let tasks: Vec<_> = (0..8)
                .map(|n| {
                    let store = Arc::clone(&store);
                    let owner = owner.clone();
                    tokio::spawn(async move {
                        let mine = server_at(
                            "00000000aa",
                            &format!("Writer {n}"),
                            "https://mcp.example.com/mcp",
                            UserAuth::None,
                        );
                        (mine.clone(), store.insert(&owner, &mine).await)
                    })
                })
                .collect();
            let mut winners = Vec::new();
            for task in tasks {
                let (mine, result) = task.await.expect("the task ran");
                match result {
                    Ok(revision) => winners.push((mine, revision)),
                    Err(UserToolServerStoreError::Exists) => {}
                    Err(other) => panic!("neither added nor `Exists`: {other}"),
                }
            }
            assert_eq!(winners.len(), 1, "round {round}: exactly one insert wins");
            let kept = store.get(&owner, winners[0].0.id()).await.unwrap().unwrap();
            assert_eq!(
                (kept.server(), kept.revision()),
                (&winners[0].0, winners[0].1)
            );
            assert_eq!(servers_of(store.as_ref(), &owner).await.len(), 1);
        }
    })
    .await;
}

/// Writers that edit from one revision have one winner: the others get a `Conflict`, and what is
/// kept is the winner's, whole, at the revision it was given.
pub async fn concurrent_replaces_from_one_revision_have_one_winner<S: UserToolServerStore>(
    store: S,
) {
    within(async {
        let store = Arc::new(store);
        for round in 0..RACE_ROUNDS {
            let owner = owner();
            let first = server("00000000aa");
            let r1 = store.insert(&owner, &first).await.unwrap();
            let tasks: Vec<_> = (0..8)
                .map(|n| {
                    let store = Arc::clone(&store);
                    let owner = owner.clone();
                    tokio::spawn(async move {
                        let mine = server_at(
                            "00000000aa",
                            &format!("Writer {n}"),
                            "https://mcp.example.com/mcp",
                            UserAuth::Bearer,
                        );
                        (mine.clone(), store.replace(&owner, &mine, r1).await)
                    })
                })
                .collect();
            let mut winners = Vec::new();
            for task in tasks {
                let (mine, result) = task.await.expect("the task ran");
                match result {
                    Ok(revision) => winners.push((mine, revision)),
                    Err(UserToolServerStoreError::Conflict) => {}
                    Err(other) => panic!("neither replaced nor a conflict: {other}"),
                }
            }
            assert_eq!(winners.len(), 1, "round {round}: exactly one replace wins");
            let kept = store.get(&owner, first.id()).await.unwrap().unwrap();
            assert_eq!(
                (kept.server(), kept.revision()),
                (&winners[0].0, winners[0].1)
            );
        }
    })
    .await;
}

/// An edit that races a delete never brings the server back: whichever lands first, the definition
/// is gone when both are done.
pub async fn a_replace_racing_a_delete_never_brings_the_server_back<S: UserToolServerStore>(
    store: S,
) {
    within(async {
        let store = Arc::new(store);
        for round in 0..RACE_ROUNDS {
            let owner = owner();
            let definition = server_with("00000000aa", UserAuth::Bearer);
            let revision = store.insert(&owner, &definition).await.unwrap();
            let edit = server_at(
                "00000000aa",
                "Edited",
                "https://mcp.example.com/mcp",
                UserAuth::Bearer,
            );
            let replace = {
                let (store, owner) = (Arc::clone(&store), owner.clone());
                tokio::spawn(async move { store.replace(&owner, &edit, revision).await })
            };
            let state = {
                let (store, owner, id) =
                    (Arc::clone(&store), owner.clone(), definition.id().clone());
                tokio::spawn(async move {
                    store
                        .set_state(&owner, &id, UserToolServerState::Connected, revision)
                        .await
                })
            };
            let delete = {
                let (store, owner, id) =
                    (Arc::clone(&store), owner.clone(), definition.id().clone());
                tokio::spawn(async move { store.delete(&owner, &id).await })
            };
            // each write either landed before the delete, was refused because the other write
            // landed first, or found nothing: never anything else
            for result in [
                replace.await.expect("the task ran").map(|_| ()),
                state.await.expect("the task ran").map(|_| ()),
            ] {
                match result {
                    Ok(())
                    | Err(
                        UserToolServerStoreError::Conflict | UserToolServerStoreError::NotFound,
                    ) => {}
                    Err(other) => panic!("round {round}: {other}"),
                }
            }
            delete.await.expect("the task ran").expect("delete");
            assert!(
                store.get(&owner, definition.id()).await.unwrap().is_none(),
                "round {round}: the deleted server is back"
            );
        }
    })
    .await;
}

/// `set_state` changes the state and nothing else; every state can be set from every other; a
/// definition that is not there is `NotFound`; each is a new revision.
pub async fn set_state_changes_the_state_only<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let definition = server_with("00000000aa", UserAuth::api_key("X-Key").unwrap())
            .with_credential(CredentialRef::new("cred-1").unwrap());
        let mut revision = store.insert(&owner, &definition).await.unwrap();
        for state in [
            UserToolServerState::Connected,
            UserToolServerState::NeedsReauth,
            UserToolServerState::Revoked,
            UserToolServerState::Connected,
            UserToolServerState::Added,
        ] {
            let next = store
                .set_state(&owner, definition.id(), state, revision)
                .await
                .expect("set_state");
            assert!(next > revision);
            revision = next;
            let got = store.get(&owner, definition.id()).await.unwrap().unwrap();
            assert_eq!(got.server().state(), state);
            assert_eq!(got.server(), &definition.clone().with_state(state));
            assert_eq!(got.revision(), revision);
        }

        let missing = UserToolServerId::parse("u-00000000zz").unwrap();
        let error = store
            .set_state(&owner, &missing, UserToolServerState::Connected, revision)
            .await
            .expect_err("nothing to change");
        assert_eq!(error.class(), ErrorClass::NotFound, "{error}");
        assert!(store.get(&owner, &missing).await.unwrap().is_none());
    })
    .await;
}

/// `delete` removes the definition and only that one, and removing what is not there succeeds.
pub async fn delete_removes_the_server_and_is_idempotent<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let (a, b) = (server("00000000aa"), server("00000000bb"));
        store.insert(&owner, &a).await.unwrap();
        store.insert(&owner, &b).await.unwrap();

        store.delete(&owner, a.id()).await.expect("delete");
        assert!(store.get(&owner, a.id()).await.unwrap().is_none());
        assert_eq!(
            store.get(&owner, b.id()).await.unwrap().unwrap().server(),
            &b
        );
        assert_eq!(servers_of(&store, &owner).await.len(), 1);

        store
            .delete(&owner, a.id())
            .await
            .expect("the second delete");
        let never = UserToolServerId::parse("u-00000000zz").unwrap();
        store
            .delete(&owner, &never)
            .await
            .expect("delete of what never was");
        assert_eq!(servers_of(&store, &owner).await.len(), 1);
    })
    .await;
}
