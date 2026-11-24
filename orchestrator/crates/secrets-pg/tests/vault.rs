//! What the Postgres vault promises beyond the port: where the rows are, that a seal is bound to its
//! `(owner, name, version)` and to the vault's own domain, that the grace is the **store's** clock
//! and the database's rows are only removed by `purge`, and that the writers of one name are
//! serialized by a lock of the vault's own. Skipped unless `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use orch_core::{Classify as _, ErrorClass};
use orch_ports::memory::FixedClock;
use orch_ports::{
    SUPERSEDED_GRACE, SecretMaterial, SecretServer, SecretStore as _, SecretVersion,
    UserToolServerId, VaultError, VaultName, VaultRef, VaultStore,
};
use orch_secrets_pg::{PgSecretStore, PgVault, advisory_lock_key, vault_advisory_lock_key};
use support::{Pool, TestDb, execute, fetch_all, fetch_one, hold_lock, ring_k1, wait_for_a_waiter};

const START: i64 = 1_700_000_000;
const SENTINEL: &str = "sentinel-6f21c8a0-this-must-not-be-in-the-table";

/// A schema of its own, its store migrated, and a vault whose grace runs on the test's clock.
struct Fixture {
    db: TestDb,
    store: PgSecretStore,
    vault: PgVault,
    clock: FixedClock,
    raw: Pool,
}

async fn setup() -> Option<Fixture> {
    let db = TestDb::new().await?;
    let store = db.store(ring_k1()).await;
    let clock = FixedClock::new(Timestamp::from_second(START).unwrap());
    let vault = PgVault::new(store.clone(), Arc::new(clock.clone()));
    let raw = db.pool("orch-vault-raw", 4).await;
    Some(Fixture {
        db,
        store,
        vault,
        clock,
        raw,
    })
}

fn name(text: &str) -> VaultName {
    VaultName::parse(text).unwrap()
}

fn material(text: &str) -> SecretMaterial {
    SecretMaterial::new(text.as_bytes()).unwrap()
}

fn start() -> Timestamp {
    Timestamp::from_second(START).unwrap()
}

fn grace() -> Duration {
    Duration::from_secs(u64::try_from(SUPERSEDED_GRACE.as_secs()).unwrap())
}

/// Every row of the table, as it is.
type RawRow = (
    String,
    String,
    i64,
    i16,
    String,
    Vec<u8>,
    Vec<u8>,
    Timestamp,
    Option<Timestamp>,
);

async fn raw_rows(pool: &Pool) -> Vec<RawRow> {
    fetch_all(
        pool,
        "SELECT owner, name, version, v, kid, nonce, ciphertext, created, superseded \
         FROM vault_secrets ORDER BY owner, name, version",
        &[],
    )
    .await
}

async fn row_count(pool: &Pool) -> i64 {
    let (count,): (i64,) = fetch_one(pool, "SELECT count(*) FROM vault_secrets", &[]).await;
    count
}

/// A write is one sealed row, at the instant the store's clock says, and the material is nowhere in
/// it.
#[tokio::test]
async fn a_secret_is_one_sealed_row_at_the_stores_clock() {
    let Some(fx) = setup().await else {
        return;
    };
    let alice = fx.db.owner("alice");
    let reference = fx
        .vault
        .put(&alice, &name("prod"), &material(SENTINEL))
        .await
        .unwrap();
    assert_eq!(reference.to_string(), "vault:prod#v1");

    let rows = raw_rows(&fx.raw).await;
    assert_eq!(rows.len(), 1, "one row per version");
    let (owner, stored_name, version, format, kid, nonce, ciphertext, created, superseded) =
        &rows[0];
    assert_eq!(owner.as_str(), alice.as_str());
    assert_eq!(stored_name, "prod");
    assert_eq!((*version, *format), (1, 1));
    assert_eq!(kid, "k1");
    assert_eq!(nonce.len(), 12);
    assert!(ciphertext.len() >= 16);
    assert_eq!(
        *created,
        start(),
        "the write is the store's clock's instant"
    );
    assert!(superseded.is_none(), "the only version is the current one");

    // nothing of the material, as it is or in hex, is in the row
    let hex: String = SENTINEL.bytes().map(|b| format!("{b:02x}")).collect();
    let all: Vec<u8> = [nonce.clone(), ciphertext.clone()].concat();
    for needle in [SENTINEL.as_bytes(), hex.as_bytes()] {
        assert!(
            !all.windows(needle.len()).any(|w| w == needle),
            "the material is in the row"
        );
    }
    assert!(!format!("{:?}", fx.vault).contains(SENTINEL));
}

/// A rotation marks the row it replaces superseded, at the instant of the write that replaces it,
/// and leaves exactly one current version.
#[tokio::test]
async fn a_rotation_supersedes_the_row_it_replaces_and_keeps_one_current() {
    let Some(fx) = setup().await else {
        return;
    };
    let alice = fx.db.owner("alice");
    let name = name("rotating");
    let first = fx.vault.put(&alice, &name, &material("one")).await.unwrap();
    fx.clock.advance(Duration::from_secs(3_600));
    let second = fx.vault.put(&alice, &name, &material("two")).await.unwrap();
    assert_eq!((first.version().get(), second.version().get()), (1, 2));

    let rows = raw_rows(&fx.raw).await;
    assert_eq!(rows.len(), 2);
    let (.., first_created, first_superseded) = &rows[0];
    let (.., second_created, second_superseded) = &rows[1];
    assert_eq!(*first_created, start());
    assert_eq!(
        *first_superseded,
        Some(start() + jiff::SignedDuration::from_hours(1)),
        "superseded at the instant of the write that replaced it"
    );
    assert_eq!(*second_created, first_superseded.unwrap());
    assert!(second_superseded.is_none());

    let (current,): (i64,) = fetch_one(
        &fx.raw,
        "SELECT count(*) FROM vault_secrets WHERE superseded IS NULL",
        &[],
    )
    .await;
    assert_eq!(current, 1, "one current version per name, at every commit");
}

/// The grace is the store's clock, and it is **fail closed**: past it the version is refused while
/// its row is still in the table, and `purge` is what removes the row — after which the answer is
/// the same.
#[tokio::test]
async fn a_version_past_its_grace_is_refused_before_its_row_is_purged() {
    let Some(fx) = setup().await else {
        return;
    };
    let alice = fx.db.owner("alice");
    let name = name("expiring");
    let old = fx.vault.put(&alice, &name, &material("old")).await.unwrap();
    let current = fx.vault.put(&alice, &name, &material("new")).await.unwrap();

    // one second before the grace is over: it reads
    fx.clock.advance(grace() - Duration::from_secs(1));
    assert_eq!(fx.vault.get(&alice, &old).await.unwrap().expose(), b"old");
    assert_eq!(row_count(&fx.raw).await, 2);

    // the instant it is over: refused, and the row is still there
    fx.clock.advance(Duration::from_secs(1));
    let error = fx.vault.get(&alice, &old).await.unwrap_err();
    assert!(matches!(error, VaultError::UnknownVersion), "{error:?}");
    assert_eq!(error.class(), ErrorClass::NotFound);
    assert_eq!(
        row_count(&fx.raw).await,
        2,
        "a read refuses the stale value whether or not the row is here"
    );

    // the housekeeping removes exactly that row
    assert_eq!(fx.vault.purge().await.unwrap(), 1);
    assert_eq!(row_count(&fx.raw).await, 1);
    assert!(matches!(
        fx.vault.get(&alice, &old).await,
        Err(VaultError::UnknownVersion)
    ));
    // and the current version is untouched
    assert_eq!(
        fx.vault.get(&alice, &current).await.unwrap().expose(),
        b"new"
    );
    assert_eq!(fx.vault.purge().await.unwrap(), 0);
}

/// A seal is bound to its owner, its name, its version **and** to the vault: a row moved to another
/// of them does not open, and neither does one copied into `user_secrets`.
#[tokio::test]
async fn a_row_moved_elsewhere_does_not_open() {
    let Some(fx) = setup().await else {
        return;
    };
    let (alice, bob) = (fx.db.owner("alice"), fx.db.owner("bob"));

    // another name
    let moved = name("moved");
    let original = fx
        .vault
        .put(&alice, &name("original"), &material("moved"))
        .await
        .unwrap();
    execute(
        &fx.raw,
        "UPDATE vault_secrets SET name = $1",
        &[&moved.as_str()],
    )
    .await;
    assert!(matches!(
        fx.vault
            .get(&alice, &VaultRef::new(moved.clone(), original.version()))
            .await,
        Err(VaultError::Corrupt(_))
    ));
    // and the name it was taken from has nothing left to read
    assert!(matches!(
        fx.vault.get(&alice, &original).await,
        Err(VaultError::UnknownName)
    ));

    // another version
    let renamed = name("renamed");
    let reference = fx
        .vault
        .put(&alice, &renamed, &material("version moved"))
        .await
        .unwrap();
    execute(&fx.raw, "UPDATE vault_secrets SET version = 99", &[]).await;
    assert!(matches!(
        fx.vault
            .get(&alice, &VaultRef::new(renamed.clone(), reference.version()))
            .await,
        Err(VaultError::UnknownVersion)
    ));
    assert!(matches!(
        fx.vault
            .get(&alice, &VaultRef::new(renamed, SecretVersion::new(99)))
            .await,
        Err(VaultError::Corrupt(_))
    ));

    // another owner: bob's copy of alice's row
    let hers = name("hers");
    fx.vault
        .put(&alice, &hers, &material("alice's"))
        .await
        .unwrap();
    execute(
        &fx.raw,
        "INSERT INTO vault_secrets (owner, name, version, v, kid, nonce, ciphertext, created) \
         SELECT $1, name, version, v, kid, nonce, ciphertext, created FROM vault_secrets \
         WHERE owner = $2",
        &[&bob.as_str(), &alice.as_str()],
    )
    .await;
    assert!(matches!(
        fx.vault
            .get(&bob, &VaultRef::new(hers.clone(), SecretVersion::new(1)))
            .await,
        Err(VaultError::Corrupt(_))
    ));
    assert_eq!(
        fx.vault
            .get(&alice, &VaultRef::new(hers.clone(), SecretVersion::new(1)))
            .await
            .unwrap()
            .expose(),
        b"alice's"
    );

    // the other table: a vault row copied into `user_secrets` under a server of the same text
    let server_id = UserToolServerId::parse("u-0123456789").unwrap();
    let shared = VaultName::parse(server_id.as_str()).unwrap();
    fx.vault
        .put(&alice, &shared, &material("vault's"))
        .await
        .unwrap();
    execute(
        &fx.raw,
        "INSERT INTO user_secrets (owner, server, version, v, kid, nonce, ciphertext) \
         SELECT owner, name, version, v, kid, nonce, ciphertext FROM vault_secrets \
         WHERE name = $1",
        &[&shared.as_str()],
    )
    .await;
    let error = fx
        .store
        .get(&alice, &SecretServer::credential(&server_id))
        .await
        .unwrap_err();
    assert!(
        matches!(error, orch_ports::SecretError::Corrupt(_)),
        "{error:?}"
    );
}

/// The vault's lock is its own: a writer of one named secret queues behind it, and a tool server's
/// credential of the same text does not.
#[tokio::test]
async fn writers_of_a_name_queue_behind_the_vaults_lock_alone() {
    let Some(fx) = setup().await else {
        return;
    };
    let alice = fx.db.owner("alice");
    let server_id = UserToolServerId::parse("u-0123456789").unwrap();
    let shared = VaultName::parse(server_id.as_str()).unwrap();
    let credential = SecretServer::credential(&server_id);
    let key = vault_advisory_lock_key(&alice, &shared);
    assert_ne!(
        key,
        advisory_lock_key(&alice, &credential),
        "the two domains never share a lock"
    );

    let held = hold_lock(&fx.raw, key).await;
    let mine = {
        let (vault, alice, shared) = (fx.vault.clone(), alice.clone(), shared.clone());
        tokio::spawn(async move { vault.put(&alice, &shared, &material("queued")).await })
    };
    wait_for_a_waiter(&fx.raw, key).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!mine.is_finished(), "the write went on without its lock");

    // the same text, as a tool server's credential, is not blocked by the vault's lock
    fx.store
        .put(&alice, &credential, &material("credential"))
        .await
        .expect("another domain's lock is not this one");

    held.commit().await.unwrap();
    let reference = mine.await.unwrap().unwrap();
    assert_eq!(reference.version().get(), 1);
    assert_eq!(
        fx.vault.get(&alice, &reference).await.unwrap().expose(),
        b"queued"
    );
}
