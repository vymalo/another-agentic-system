//! Runs the `SecretStore` conformance testkit against the in-memory store, and pins what the
//! in-memory store adds to it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::atomic::{AtomicBool, Ordering};

use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::memory::MemorySecretStore;
use orch_ports::{
    SecretError, SecretMaterial, SecretServer, SecretStore, SecretVersion, UserToolServerId,
};

async fn make() -> Option<MemorySecretStore> {
    Some(MemorySecretStore::new())
}

/// What the memory store supplies to break itself: it is taken down.
async fn break_the_store(store: &MemorySecretStore) {
    store.set_down(true);
}

orch_ports::secret_store_conformance!(make, breaker = break_the_store);

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

// ---------------------------------------------------- the case that looks into errors can fail

/// How a store that fails can leak the material it was given into the error it answers with.
#[derive(Clone, Copy)]
enum Leak {
    /// Nothing: a store that is not leaky.
    No,
    /// In the text of the error.
    InTheDetail,
    /// In the text of the error, in hex.
    InTheDetailInHex,
    /// In the error that caused it.
    InTheSource,
}

/// The memory store, whole until it is broken, and then leaking `how` into what it answers with.
struct Leaky {
    inner: MemorySecretStore,
    broken: AtomicBool,
    how: Leak,
}

impl Leaky {
    fn new(how: Leak) -> Self {
        Leaky {
            inner: MemorySecretStore::new(),
            broken: AtomicBool::new(false),
            how,
        }
    }

    fn failure(&self, material: &[u8]) -> SecretError {
        let text = String::from_utf8_lossy(material).into_owned();
        match self.how {
            Leak::No => SecretError::unavailable("the store is down"),
            Leak::InTheDetail => SecretError::unavailable(format!("could not write {text}")),
            Leak::InTheDetailInHex => {
                let hex: String = material.iter().map(|b| format!("{b:02x}")).collect();
                SecretError::unavailable(format!("could not write {hex}"))
            }
            Leak::InTheSource => SecretError::unavailable("could not write")
                .with_source(std::io::Error::other(format!("rejected {text}"))),
        }
    }
}

impl SecretStore for Leaky {
    async fn put(
        &self,
        owner: &UserId,
        server: &SecretServer,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(self.failure(material.expose()));
        }
        self.inner.put(owner, server, material).await
    }

    async fn get(
        &self,
        owner: &UserId,
        server: &SecretServer,
    ) -> Result<Option<(SecretVersion, SecretMaterial)>, SecretError> {
        if self.broken.load(Ordering::SeqCst) {
            // the store reads its own row to fail on it, and leaks what it read
            let kept = self.inner.get(owner, server).await?;
            let bytes = kept.map(|(_, m)| m.expose().to_vec()).unwrap_or_default();
            return Err(self.failure(&bytes));
        }
        self.inner.get(owner, server).await
    }

    async fn delete(&self, owner: &UserId, server: &SecretServer) -> Result<(), SecretError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(self.failure(b""));
        }
        self.inner.delete(owner, server).await
    }

    async fn rotate(
        &self,
        owner: &UserId,
        server: &SecretServer,
        expected: SecretVersion,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(self.failure(material.expose()));
        }
        self.inner.rotate(owner, server, expected, material).await
    }
}

async fn break_leaky(store: &Leaky) {
    store.broken.store(true, Ordering::SeqCst);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_store_that_does_not_leak_passes_the_case() {
    orch_ports::testkit::secret_store::no_error_from_a_broken_store_shows_a_secret(
        Leaky::new(Leak::No),
        break_leaky,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "the secret is in")]
async fn a_secret_in_the_text_of_an_error_fails_the_case() {
    orch_ports::testkit::secret_store::no_error_from_a_broken_store_shows_a_secret(
        Leaky::new(Leak::InTheDetail),
        break_leaky,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "the secret, in hex, is in")]
async fn a_secret_in_hex_in_an_error_fails_the_case() {
    orch_ports::testkit::secret_store::no_error_from_a_broken_store_shows_a_secret(
        Leaky::new(Leak::InTheDetailInHex),
        break_leaky,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "the secret is in")]
async fn a_secret_in_the_source_of_an_error_fails_the_case() {
    orch_ports::testkit::secret_store::no_error_from_a_broken_store_shows_a_secret(
        Leaky::new(Leak::InTheSource),
        break_leaky,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "a broken store cannot put")]
async fn a_breaker_that_breaks_nothing_fails_the_case() {
    async fn break_nothing(_: &MemorySecretStore) {}
    orch_ports::testkit::secret_store::no_error_from_a_broken_store_shows_a_secret(
        MemorySecretStore::new(),
        break_nothing,
    )
    .await;
}
