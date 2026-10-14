//! `UserToolServerStore` conformance cases: what the API and the relay rely on from any store of a
//! person's MCP server definitions (ADR 0046), stated once and run against the in-memory store and
//! the Postgres store.
//!
//! An implementation supplies a fresh store (`async fn() -> Option<S>`; `None` skips the suite).
//! Each case is `async fn(store)`, uses owners of its own (so a store shared by the cases of one
//! run is never confused) and gives up after 60 seconds.
//!
//! What is deliberately not asserted: how a definition is kept, and the wording of any error.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use orch_core::{Classify, ErrorClass, UserId};
use uuid::Uuid;

use crate::{
    CredentialRef, MAX_USER_TOOL_SERVERS, UserAuth, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError,
};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A person no other case uses.
fn owner() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

/// A definition with the id `u-<suffix>` (10 characters) and the given kind.
fn server_with(suffix: &str, auth: UserAuth) -> UserToolServer {
    let id = UserToolServerId::parse(&format!("u-{suffix}")).expect("a valid id");
    UserToolServer::new(
        id,
        format!("Server {suffix}"),
        "https://mcp.example.com/mcp",
        auth,
    )
    .expect("a valid definition")
}

fn server(suffix: &str) -> UserToolServer {
    server_with(suffix, UserAuth::None)
}

/// The id `u-` and ten digits of `n`.
fn id_of(n: usize) -> String {
    format!("{n:010}")
}

/// A definition of every kind comes back whole: id, name, URL, kind with its header, scopes and
/// client id, state and credential reference.
pub async fn put_then_get_round_trips<S: UserToolServerStore>(store: S) {
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
            store.put(&owner, &definition).await.expect("put");
            let got = store
                .get(&owner, definition.id())
                .await
                .expect("get")
                .expect("the definition is kept");
            assert_eq!(got, definition);
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
        store.put(&owner, &definition).await.expect("put");
        let got = store.get(&owner, definition.id()).await.unwrap().unwrap();
        assert_eq!(got.state(), UserToolServerState::Added);
        assert!(got.credential().is_none());
    })
    .await;
}

/// `list` returns the owner's definitions, ordered by id, and nobody else's.
pub async fn list_returns_the_owners_servers_ordered_by_id<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        // put out of order, so an order of insertion is not mistaken for the order of ids
        for suffix in ["00000000cc", "00000000aa", "00000000bb"] {
            store.put(&owner, &server(suffix)).await.expect("put");
        }
        let ids: Vec<String> = store
            .list(&owner)
            .await
            .expect("list")
            .iter()
            .map(|s| s.id().to_string())
            .collect();
        assert_eq!(ids, ["u-00000000aa", "u-00000000bb", "u-00000000cc"]);
    })
    .await;
}

/// Nobody reads, lists, changes or removes another person's definition: for the other person it is
/// not there.
pub async fn another_owner_sees_nothing<S: UserToolServerStore>(store: S) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let definition =
            server_with("00000000aa", UserAuth::Bearer).with_state(UserToolServerState::Connected);
        store.put(&alice, &definition).await.expect("put");

        assert!(store.get(&bob, definition.id()).await.unwrap().is_none());
        assert!(store.list(&bob).await.unwrap().is_empty());
        let error = store
            .set_state(&bob, definition.id(), UserToolServerState::Revoked)
            .await
            .expect_err("bob cannot change alice's server");
        assert_eq!(error.class(), ErrorClass::NotFound, "{error}");
        store
            .delete(&bob, definition.id())
            .await
            .expect("delete is idempotent");

        // and none of it touched alice's
        let kept = store.get(&alice, definition.id()).await.unwrap().unwrap();
        assert_eq!(kept, definition);
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
        store.put(&alice, &a).await.unwrap();
        store.put(&bob, &b).await.unwrap();
        assert_eq!(store.get(&alice, a.id()).await.unwrap().unwrap(), a);
        assert_eq!(store.get(&bob, b.id()).await.unwrap().unwrap(), b);
        store.delete(&alice, a.id()).await.unwrap();
        assert!(store.get(&alice, a.id()).await.unwrap().is_none());
        assert_eq!(store.get(&bob, b.id()).await.unwrap().unwrap(), b);
    })
    .await;
}

/// `put` of an id that exists replaces the definition whole (the edit): nothing of the old one is
/// kept, and the number of definitions does not change.
pub async fn put_replaces_an_existing_server_whole<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let first = server_with("00000000aa", UserAuth::api_key("X-Key").unwrap())
            .with_state(UserToolServerState::Connected)
            .with_credential(CredentialRef::new("cred-old").unwrap());
        store.put(&owner, &first).await.unwrap();

        let second = UserToolServer::new(
            first.id().clone(),
            "Renamed",
            "https://other.example.com/mcp",
            UserAuth::Bearer,
        )
        .unwrap();
        store.put(&owner, &second).await.unwrap();

        let got = store.get(&owner, first.id()).await.unwrap().unwrap();
        assert_eq!(got, second);
        assert_eq!(got.name(), "Renamed");
        assert!(got.credential().is_none());
        assert_eq!(got.state(), UserToolServerState::Added);
        assert_eq!(store.list(&owner).await.unwrap().len(), 1);
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
        for n in 0..MAX_USER_TOOL_SERVERS {
            store
                .put(&owner, &server(&id_of(n)))
                .await
                .unwrap_or_else(|e| panic!("server {n} is within the limit: {e}"));
        }
        let ninth = server(&id_of(MAX_USER_TOOL_SERVERS));
        let error = store
            .put(&owner, &ninth)
            .await
            .expect_err("a ninth server is refused");
        assert!(
            matches!(error, UserToolServerStoreError::TooMany { limit: 8 }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Rejected);
        assert!(store.get(&owner, ninth.id()).await.unwrap().is_none());
        assert_eq!(
            store.list(&owner).await.unwrap().len(),
            MAX_USER_TOOL_SERVERS
        );

        // an edit of one of the eight is not a ninth
        let edit = server_with(&id_of(3), UserAuth::Bearer);
        store
            .put(&owner, &edit)
            .await
            .expect("an edit at the limit");
        assert_eq!(
            store.list(&owner).await.unwrap().len(),
            MAX_USER_TOOL_SERVERS
        );

        // another person is unaffected, and the first can add again after a removal
        store
            .put(&other, &ninth)
            .await
            .expect("another owner's count is their own");
        store.delete(&owner, edit.id()).await.unwrap();
        store
            .put(&owner, &ninth)
            .await
            .expect("room after a removal");
        assert_eq!(
            store.list(&owner).await.unwrap().len(),
            MAX_USER_TOOL_SERVERS
        );
    })
    .await;
}

/// Writers that race for the last places never pass the limit: of sixteen concurrent adds, eight
/// succeed and eight are refused for being too many.
pub async fn concurrent_puts_never_pass_the_limit<S: UserToolServerStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        let owner = owner();
        let tasks: Vec<_> = (0..2 * MAX_USER_TOOL_SERVERS)
            .map(|n| {
                let store = Arc::clone(&store);
                let owner = owner.clone();
                tokio::spawn(async move { store.put(&owner, &server(&id_of(n))).await })
            })
            .collect();
        let (mut ok, mut refused) = (0, 0);
        for task in tasks {
            match task.await.expect("the task ran") {
                Ok(()) => ok += 1,
                Err(UserToolServerStoreError::TooMany { .. }) => refused += 1,
                Err(other) => panic!("neither added nor refused for being too many: {other}"),
            }
        }
        assert_eq!(
            (ok, refused),
            (MAX_USER_TOOL_SERVERS, MAX_USER_TOOL_SERVERS)
        );
        assert_eq!(
            store.list(&owner).await.unwrap().len(),
            MAX_USER_TOOL_SERVERS
        );
    })
    .await;
}

/// `set_state` changes the state and nothing else; every state can be set from every other; a
/// definition that is not there is `NotFound`.
pub async fn set_state_changes_the_state_only<S: UserToolServerStore>(store: S) {
    within(async {
        let owner = owner();
        let definition = server_with("00000000aa", UserAuth::api_key("X-Key").unwrap())
            .with_credential(CredentialRef::new("cred-1").unwrap());
        store.put(&owner, &definition).await.unwrap();
        for state in [
            UserToolServerState::Connected,
            UserToolServerState::NeedsReauth,
            UserToolServerState::Revoked,
            UserToolServerState::Connected,
            UserToolServerState::Added,
        ] {
            store
                .set_state(&owner, definition.id(), state)
                .await
                .expect("set_state");
            let got = store.get(&owner, definition.id()).await.unwrap().unwrap();
            assert_eq!(got.state(), state);
            assert_eq!(got, definition.clone().with_state(state));
        }

        let missing = UserToolServerId::parse("u-00000000zz").unwrap();
        let error = store
            .set_state(&owner, &missing, UserToolServerState::Connected)
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
        store.put(&owner, &a).await.unwrap();
        store.put(&owner, &b).await.unwrap();

        store.delete(&owner, a.id()).await.expect("delete");
        assert!(store.get(&owner, a.id()).await.unwrap().is_none());
        assert_eq!(store.get(&owner, b.id()).await.unwrap().unwrap(), b);
        assert_eq!(store.list(&owner).await.unwrap().len(), 1);

        store
            .delete(&owner, a.id())
            .await
            .expect("the second delete");
        let never = UserToolServerId::parse("u-00000000zz").unwrap();
        store
            .delete(&owner, &never)
            .await
            .expect("delete of what never was");
        assert_eq!(store.list(&owner).await.unwrap().len(), 1);
    })
    .await;
}
