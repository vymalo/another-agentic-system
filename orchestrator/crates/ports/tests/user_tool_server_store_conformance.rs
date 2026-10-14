//! Runs the `UserToolServerStore` conformance testkit against the in-memory store, and pins what
//! the in-memory store adds to it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::memory::MemoryUserToolServers;
use orch_ports::{
    EgressPolicy, ServerRevision, UserAuth, UserToolServer, UserToolServerId, UserToolServerState,
    UserToolServerStore, UserToolServerStoreError,
};

async fn make() -> Option<MemoryUserToolServers> {
    Some(MemoryUserToolServers::new())
}

orch_ports::user_tool_server_store_conformance!(make);

fn definition(suffix: &str) -> UserToolServer {
    let target = EgressPolicy::new()
        .check_url("https://mcp.example.com/mcp")
        .unwrap();
    UserToolServer::new(
        UserToolServerId::parse(&format!("u-{suffix}")).unwrap(),
        "Server",
        &target,
        UserAuth::None,
    )
    .unwrap()
}

#[tokio::test]
async fn clones_share_the_definitions_and_the_store_can_be_looked_into() {
    let store = MemoryUserToolServers::new();
    let clone = store.clone();
    let owner = UserId::new("alice@example.test");
    assert!(store.is_empty());
    clone
        .insert(&owner, &definition("0000000001"))
        .await
        .unwrap();
    assert_eq!(store.len(), 1);
    assert!(!store.is_empty());
    store
        .delete(&owner, definition("0000000001").id())
        .await
        .unwrap();
    assert!(clone.is_empty());
}

#[tokio::test]
async fn a_store_that_is_down_is_unavailable_until_it_is_up() {
    let store = MemoryUserToolServers::new();
    let owner = UserId::new("alice@example.test");
    let revision = store
        .insert(&owner, &definition("0000000001"))
        .await
        .unwrap();
    store.set_down(true);
    let id = definition("0000000001").id().clone();
    let errors = [
        store
            .insert(&owner, &definition("0000000002"))
            .await
            .map(|_| ())
            .unwrap_err(),
        store.get(&owner, &id).await.map(|_| ()).unwrap_err(),
        store.list(&owner).await.map(|_| ()).unwrap_err(),
        store
            .replace(&owner, &definition("0000000001"), revision)
            .await
            .map(|_| ())
            .unwrap_err(),
        store
            .set_state(&owner, &id, UserToolServerState::Revoked, revision)
            .await
            .map(|_| ())
            .unwrap_err(),
        store.delete(&owner, &id).await.unwrap_err(),
    ];
    for error in errors {
        assert!(
            matches!(error, UserToolServerStoreError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);
    }
    store.set_down(false);
    assert_eq!(
        store.list(&owner).await.unwrap().len(),
        1,
        "nothing was written while it was down"
    );
}

#[tokio::test]
async fn revisions_are_counted_for_the_whole_store_so_a_deleted_one_is_never_repeated() {
    let store = MemoryUserToolServers::new();
    let owner = UserId::new("alice@example.test");
    let a = store
        .insert(&owner, &definition("0000000001"))
        .await
        .unwrap();
    let b = store
        .insert(&owner, &definition("0000000002"))
        .await
        .unwrap();
    store
        .delete(&owner, definition("0000000001").id())
        .await
        .unwrap();
    let c = store
        .insert(&owner, &definition("0000000001"))
        .await
        .unwrap();
    assert!(a < b && b < c);
    assert_ne!(a, ServerRevision::new(0));
}
