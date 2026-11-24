//! Runs the `VaultStore` conformance testkit against the in-memory vault, and proves that the case
//! which looks into errors can fail.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use jiff::Timestamp;
use orch_core::UserId;
use orch_ports::memory::{FixedClock, MemoryVault};
use orch_ports::{SecretMaterial, VaultError, VaultName, VaultRef, VaultStore};

/// A clock every case of one store shares, at a whole second (Postgres keeps microseconds, so a
/// fixture that is right for both adapters starts there).
fn clock() -> FixedClock {
    FixedClock::new(Timestamp::from_second(1_700_000_000).unwrap())
}

async fn make() -> Option<(MemoryVault, FixedClock)> {
    let clock = clock();
    Some((MemoryVault::new(clock.clone()), clock))
}

orch_ports::vault_store_conformance!(make);

// ------------------------------------------------------- what the memory vault adds to the kit

#[tokio::test]
async fn clones_share_the_secrets_and_the_clock() {
    let clock = clock();
    let store = MemoryVault::new(clock.clone());
    let clone = store.clone();
    let owner = UserId::new("alice@example.test");
    let name = VaultName::parse("prod").unwrap();
    let reference = store
        .put(&owner, &name, &SecretMaterial::new(b"x").unwrap())
        .await
        .unwrap();
    assert_eq!(clone.get(&owner, &reference).await.unwrap().expose(), b"x");
    // moving the clock of the handle the test kept moves it for the store it gave away
    clock.advance(std::time::Duration::from_hours(25));
    assert!(
        clone.get(&owner, &reference).await.is_ok(),
        "the current version never expires"
    );
    assert!(format!("{store:?}").contains("MemoryVault"));
}

// ------------------------------------ the case that looks into errors: proved able to fail

/// How a store that fails can leak the material it read into the error it answers with.
#[derive(Clone, Copy)]
enum Leak {
    /// Nothing: a store that is not leaky.
    No,
    /// In the text of the error.
    InTheDetail,
    /// In the text of the error, in hex.
    InTheDetailInHex,
}

/// The memory vault, and a `get` that answers with the material it was last given when it finds no
/// such version: a store that keeps what a caller can already read to itself.
struct Leaky {
    inner: MemoryVault,
    last: std::sync::Mutex<Vec<u8>>,
    leak: Leak,
}

impl Leaky {
    /// A leaky vault **whose grace runs on `clock`**: the clock the case is given, not another.
    fn new(leak: Leak, clock: FixedClock) -> Self {
        Leaky {
            inner: MemoryVault::new(clock),
            last: std::sync::Mutex::new(Vec::new()),
            leak,
        }
    }

    fn leak(&self) -> VaultError {
        let kept = self
            .last
            .lock()
            .map(|last| last.clone())
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&kept).into_owned();
        match self.leak {
            Leak::No => VaultError::UnknownVersion,
            Leak::InTheDetail => VaultError::corrupt(format!("no such version; kept {text}")),
            Leak::InTheDetailInHex => {
                let hex: String = kept.iter().map(|b| format!("{b:02x}")).collect();
                VaultError::corrupt(format!("no such version; kept {hex}"))
            }
        }
    }
}

impl VaultStore for Leaky {
    async fn put(
        &self,
        owner: &UserId,
        name: &VaultName,
        material: &SecretMaterial,
    ) -> Result<VaultRef, VaultError> {
        if let Ok(mut last) = self.last.lock() {
            *last = material.expose().to_vec();
        }
        self.inner.put(owner, name, material).await
    }

    async fn get(
        &self,
        owner: &UserId,
        reference: &VaultRef,
    ) -> Result<SecretMaterial, VaultError> {
        match self.inner.get(owner, reference).await {
            Err(VaultError::UnknownVersion) => Err(self.leak()),
            other => other,
        }
    }

    async fn purge(&self) -> Result<u64, VaultError> {
        self.inner.purge().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_that_does_not_leak_passes_the_case() {
    let clock = clock();
    orch_ports::testkit::vault_store::no_error_or_debug_text_shows_a_secret(
        Leaky::new(Leak::No, clock.clone()),
        clock,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "the secret is in")]
async fn a_secret_in_the_text_of_an_error_fails_the_case() {
    let clock = clock();
    orch_ports::testkit::vault_store::no_error_or_debug_text_shows_a_secret(
        Leaky::new(Leak::InTheDetail, clock.clone()),
        clock,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "the secret, in hex, is in")]
async fn a_secret_in_hex_in_an_error_fails_the_case() {
    let clock = clock();
    orch_ports::testkit::vault_store::no_error_or_debug_text_shows_a_secret(
        Leaky::new(Leak::InTheDetailInHex, clock.clone()),
        clock,
    )
    .await;
}
