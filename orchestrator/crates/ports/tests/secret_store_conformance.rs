//! Runs the `SecretStore` conformance testkit against the in-memory store, and pins what the
//! in-memory store adds to it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::memory::MemorySecretStore;
use orch_ports::{
    SecretError, SecretMaterial, SecretServer, SecretStore, SecretVersion, UserToolServerId,
};

async fn make() -> Option<MemorySecretStore> {
    Some(MemorySecretStore::new())
}

orch_ports::secret_store_conformance!(make);

fn server() -> SecretServer {
    SecretServer::credential(&UserToolServerId::parse("u-0000000001").unwrap())
}

fn material(bytes: &[u8]) -> SecretMaterial {
    SecretMaterial::new(bytes).unwrap()
}

#[tokio::test]
async fn clones_share_the_secrets_and_the_store_can_be_looked_into() {
    let store = MemorySecretStore::new();
    let clone = store.clone();
    let owner = UserId::new("alice@example.test");
    assert!(store.is_empty());
    clone.put(&owner, &server(), &material(b"x")).await.unwrap();
    assert_eq!(store.len(), 1);
    store.delete(&owner, &server()).await.unwrap();
    assert!(clone.is_empty());
}

#[tokio::test]
async fn a_store_that_is_down_is_unavailable_until_it_is_up() {
    let store = MemorySecretStore::new();
    let owner = UserId::new("alice@example.test");
    let version = store.put(&owner, &server(), &material(b"x")).await.unwrap();
    store.set_down(true);
    let errors = [
        store
            .put(&owner, &server(), &material(b"y"))
            .await
            .unwrap_err(),
        store.get(&owner, &server()).await.map(|_| ()).unwrap_err(),
        store.delete(&owner, &server()).await.unwrap_err(),
        store
            .rotate(&owner, &server(), version, &material(b"y"))
            .await
            .map(|_| ())
            .unwrap_err(),
    ];
    for error in errors {
        assert!(
            matches!(error, SecretError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);
    }
    store.set_down(false);
    let (kept, got) = store.get(&owner, &server()).await.unwrap().unwrap();
    assert_eq!(
        (kept, got.expose()),
        (version, &b"x"[..]),
        "nothing was written while it was down"
    );
}

#[tokio::test]
async fn versions_are_not_shared_in_meaning_across_keys_but_are_never_repeated() {
    // the memory store counts every write of every key from one counter
    let store = MemorySecretStore::new();
    let owner = UserId::new("alice@example.test");
    let other = SecretServer::credential(&UserToolServerId::parse("u-0000000002").unwrap());
    let a = store.put(&owner, &server(), &material(b"a")).await.unwrap();
    let b = store.put(&owner, &other, &material(b"b")).await.unwrap();
    let c = store.put(&owner, &server(), &material(b"c")).await.unwrap();
    assert!(a < b && b < c);
    assert_ne!(a, SecretVersion::new(0));
}
