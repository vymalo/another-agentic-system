//! What the Postgres adapter promises beyond the conformance testkit: where a secret is bound,
//! what the table holds, how versions are numbered, and how the migrations live beside the
//! orchestrator's. Skipped unless `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::HashSet;
use std::sync::Arc;

use orch_core::{Classify as _, ErrorClass};
use orch_ports::{SecretError, SecretMaterial, SecretServer, SecretStore as _, UserToolServerId};
use orch_secrets_pg::{MIGRATIONS_TABLE, PgSecretStore, is_outage};
use support::{Pool, TestDb, execute, fetch_all, fetch_one, ring, ring_k1};

const SENTINEL: &str = "sentinel-5d0e1c77-this-must-not-be-in-the-table";

fn credential(suffix: &str) -> SecretServer {
    SecretServer::credential(&UserToolServerId::parse(&format!("u-{suffix}")).unwrap())
}

fn flow(suffix: &str) -> SecretServer {
    SecretServer::pending_flow(&UserToolServerId::parse(&format!("u-{suffix}")).unwrap())
}

fn material(text: &str) -> SecretMaterial {
    SecretMaterial::new(text.as_bytes()).unwrap()
}

type RawRow = (String, String, i64, i16, String, Vec<u8>, Vec<u8>);

async fn raw_rows(pool: &Pool) -> Vec<RawRow> {
    fetch_all(
        pool,
        "SELECT owner, server, version, v, kid, nonce, ciphertext FROM user_secrets ORDER BY owner, server",
        &[],
    )
    .await
}

/// The orchestrator's own migrations (`orch-store-postgres`, in sqlx's `_sqlx_migrations`), read
/// from its directory and applied by the runner that continues sqlx's history: this crate's tests
/// depend on no crate that is still on sqlx.
fn orchestrators_migrator() -> orch_pg_migrate::Migrator {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../store-postgres/migrations");
    let mut migrations = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".sql") {
            let sql: &'static str = Box::leak(std::fs::read_to_string(&path).unwrap().into());
            migrations.push(orch_pg_migrate::Migration::from_file(&name, sql).unwrap());
        }
    }
    orch_pg_migrate::Migrator::new("_sqlx_migrations", migrations).unwrap()
}

/// The orchestrator's migrations on a connection of `pool`, as its store runs them at boot.
async fn migrate_threads(pool: &Pool) -> Result<(), orch_pg_migrate::MigrateError> {
    let mut conn = pool.get().await.unwrap();
    orchestrators_migrator().run(&mut conn).await
}

async fn setup() -> Option<(TestDb, PgSecretStore, Pool)> {
    let db = TestDb::new().await?;
    let store = db.store(ring_k1()).await;
    let raw = db.pool("orch-secrets-raw", 4).await;
    Some((db, store, raw))
}

#[test]
fn the_store_can_be_shared_between_tasks() {
    fn shared<T: Clone + Send + Sync + 'static>() {}
    shared::<PgSecretStore>();
}

#[tokio::test]
async fn a_row_moved_to_another_owner_does_not_open() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };

    let alice = db.owner("alice");
    let bob = db.owner("bob");
    let server = credential("0123456789");
    store
        .put(&alice, &server, &material("alice's"))
        .await
        .unwrap();

    // a copy of alice's row under bob, byte for byte
    execute(
        &raw,
        "INSERT INTO user_secrets (owner, server, version, v, kid, nonce, ciphertext) \
         SELECT $1, server, version, v, kid, nonce, ciphertext FROM user_secrets",
        &[&bob.as_str()],
    )
    .await;

    let moved = store.get(&bob, &server).await.unwrap_err();
    assert!(matches!(moved, SecretError::Corrupt(_)), "{moved:?}");
    assert_eq!(moved.class(), ErrorClass::Corrupt);
    // alice's own row still opens
    assert_eq!(
        store
            .get(&alice, &server)
            .await
            .unwrap()
            .unwrap()
            .1
            .expose(),
        b"alice's"
    );
}

#[tokio::test]
async fn a_row_moved_to_another_server_or_to_the_pending_flow_does_not_open() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };

    let alice = db.owner("alice");
    let (a, b) = (credential("aaaaaaaaaa"), credential("bbbbbbbbbb"));
    store.put(&alice, &a, &material("a")).await.unwrap();
    store.put(&alice, &b, &material("b")).await.unwrap();

    // the envelopes swapped: each row holds the other's seal
    execute(
        &raw,
        "UPDATE user_secrets t SET v = o.v, kid = o.kid, nonce = o.nonce, ciphertext = o.ciphertext \
         FROM (SELECT server, v, kid, nonce, ciphertext FROM user_secrets) o \
         WHERE t.server <> o.server",
        &[],
    )
    .await;
    for server in [&a, &b] {
        let error = store.get(&alice, server).await.unwrap_err();
        assert!(
            matches!(error, SecretError::Corrupt(_)),
            "{server}: {error:?}"
        );
    }

    // a credential's row renamed into its pending flow
    let (c, f) = (credential("cccccccccc"), flow("cccccccccc"));
    store.put(&alice, &c, &material("c")).await.unwrap();
    execute(
        &raw,
        "UPDATE user_secrets SET server = $1 WHERE server = $2",
        &[&f.to_string(), &c.to_string()],
    )
    .await;
    assert!(matches!(
        store.get(&alice, &f).await,
        Err(SecretError::Corrupt(_))
    ));
}

#[tokio::test]
async fn a_row_given_another_version_does_not_open() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };

    let alice = db.owner("alice");
    let server = credential("0123456789");
    let version = store.put(&alice, &server, &material("x")).await.unwrap();
    execute(&raw, "UPDATE user_secrets SET version = version + 1", &[]).await;
    assert!(matches!(
        store.get(&alice, &server).await,
        Err(SecretError::Corrupt(_))
    ));
    // put it back, and it opens
    execute(&raw, "UPDATE user_secrets SET version = version - 1", &[]).await;
    let (got, material) = store.get(&alice, &server).await.unwrap().unwrap();
    assert_eq!((got, material.expose()), (version, &b"x"[..]));
}

#[tokio::test]
async fn an_edited_ciphertext_nonce_key_id_or_format_is_corrupt_and_the_error_holds_no_bytes() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };

    let alice = db.owner("alice");
    let server = credential("0123456789");
    store
        .put(&alice, &server, &material(SENTINEL))
        .await
        .unwrap();

    let edits = [
        "UPDATE user_secrets SET ciphertext = set_byte(ciphertext, 0, get_byte(ciphertext, 0) # 1)",
        "UPDATE user_secrets SET nonce = set_byte(nonce, 3, get_byte(nonce, 3) # 128)",
        "UPDATE user_secrets SET v = 2",
        // a tag that is cut, and a ciphertext that is only a tag
        "UPDATE user_secrets SET ciphertext = substring(ciphertext from 1 for octet_length(ciphertext) - 1)",
    ];
    for edit in edits {
        let (reset_to,): (Vec<u8>,) =
            fetch_one(&raw, "SELECT ciphertext FROM user_secrets", &[]).await;
        let (v, nonce): (i16, Vec<u8>) =
            fetch_one(&raw, "SELECT v, nonce FROM user_secrets", &[]).await;
        execute(&raw, edit, &[]).await;
        let error = store.get(&alice, &server).await.unwrap_err();
        assert!(
            matches!(error, SecretError::Corrupt(_)),
            "{edit}: {error:?}"
        );
        let said = format!("{error} {error:?}");
        assert!(!said.contains("sentinel"), "{said}");
        let hex: String = reset_to.iter().map(|b| format!("{b:02x}")).collect();
        assert!(!said.contains(&hex[..16]), "{said}");
        execute(
            &raw,
            "UPDATE user_secrets SET v = $1, nonce = $2, ciphertext = $3",
            &[&v, &nonce, &reset_to],
        )
        .await;
        assert_eq!(
            store
                .get(&alice, &server)
                .await
                .unwrap()
                .unwrap()
                .1
                .expose(),
            SENTINEL.as_bytes(),
            "restored after {edit}"
        );
    }
}

/// `k2` holds the very bytes of `k1`, so a row whose key id is edited from `k1` to `k2` can only be
/// told from a row sealed under `k2` by the key id the seal authenticates.
#[tokio::test]
async fn a_key_id_edited_into_another_key_with_the_same_bytes_does_not_open() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let alice = db.owner("alice");
    let store = db.store(ring("k1", &[("k1", 1), ("k2", 1)])).await;
    let raw = db.pool("orch-secrets-raw", 2).await;
    let server = credential("0123456789");
    store.put(&alice, &server, &material("x")).await.unwrap();
    execute(&raw, "UPDATE user_secrets SET kid = 'k2'", &[]).await;
    let got = store.get(&alice, &server).await;
    // the outcome without the material: an error prints as its text, a found secret as its version
    let said = got
        .as_ref()
        .map(|found| found.as_ref().map(|(version, _)| version));
    assert!(matches!(got, Err(SecretError::Corrupt(_))), "{said:?}");
}

#[tokio::test]
async fn a_row_sealed_under_a_key_this_deployment_does_not_hold_is_key_unknown() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };
    let alice = db.owner("alice");
    let (a, b) = (credential("aaaaaaaaaa"), credential("bbbbbbbbbb"));
    store.put(&alice, &a, &material("a")).await.unwrap();
    store.put(&alice, &b, &material("b")).await.unwrap();

    // a deployment that has dropped k1 before the rows were re-sealed
    let without = db.another_store(ring("k2", &[("k2", 2)])).await;
    let error = without.get(&alice, &a).await.unwrap_err();
    assert!(matches!(error, SecretError::KeyUnknown), "{error:?}");
    assert_eq!(error.class(), ErrorClass::Corrupt);
    assert!(
        !format!("{error} {error:?}").contains("k1"),
        "no key id in the error"
    );

    // that server is unavailable and nothing else is: a secret it writes is readable, and a
    // `put` replaces the one it cannot read (a new authorization after a key was lost)
    let fresh = credential("cccccccccc");
    without.put(&alice, &fresh, &material("c")).await.unwrap();
    assert_eq!(
        without
            .get(&alice, &fresh)
            .await
            .unwrap()
            .unwrap()
            .1
            .expose(),
        b"c"
    );
    without.put(&alice, &a, &material("a again")).await.unwrap();
    assert_eq!(
        without.get(&alice, &a).await.unwrap().unwrap().1.expose(),
        b"a again"
    );
    // delete never needed the key
    without.delete(&alice, &b).await.unwrap();
    let kids: Vec<String> = raw_rows(&raw).await.into_iter().map(|row| row.4).collect();
    assert_eq!(kids, ["k2", "k2"]);
    // and a deployment that holds only k1 cannot read what is sealed under k2 now
    assert!(matches!(
        store.get(&alice, &a).await,
        Err(SecretError::KeyUnknown)
    ));
}

#[tokio::test]
async fn a_rotate_replaces_a_row_it_cannot_open_when_the_version_is_the_stored_one() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };
    let alice = db.owner("alice");
    let server = credential("0123456789");
    let version = store.put(&alice, &server, &material("old")).await.unwrap();
    let without = db.another_store(ring("k2", &[("k2", 2)])).await;
    // rotate compares versions and writes; it never opens the old material
    let next = without
        .rotate(&alice, &server, version, &material("new"))
        .await
        .unwrap();
    assert_eq!(without.get(&alice, &server).await.unwrap().unwrap().0, next);
    assert_eq!(raw_rows(&raw).await[0].4, "k2");
}

#[tokio::test]
async fn two_seals_of_the_same_material_differ_in_nonce_and_in_ciphertext() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };

    let alice = db.owner("alice");
    let (a, b) = (credential("aaaaaaaaaa"), credential("bbbbbbbbbb"));
    for server in [&a, &b] {
        store
            .put(&alice, server, &material("the very same material"))
            .await
            .unwrap();
    }
    // and one key sealed twice
    store
        .put(&alice, &a, &material("the very same material"))
        .await
        .unwrap();
    let rows = raw_rows(&raw).await;
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].5, rows[1].5, "nonces");
    assert_ne!(rows[0].6, rows[1].6, "ciphertexts");
    assert_eq!(rows[0].4, "k1");
    assert_eq!(rows[0].3, 1, "the format");
}

#[tokio::test]
async fn the_table_holds_no_plaintext() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };

    let alice = db.owner("alice");
    let server = credential("0123456789");
    let json = format!("{{\"access_token\":\"{SENTINEL}\",\"refresh_token\":\"{SENTINEL}-r\"}}");
    store.put(&alice, &server, &material(&json)).await.unwrap();
    store
        .put(&alice, &flow("0123456789"), &material(SENTINEL))
        .await
        .unwrap();

    // the whole row as Postgres prints it, bytea in hex
    let (text,): (String,) = fetch_one(
        &raw,
        "SELECT string_agg(t::text, ' ') FROM user_secrets t",
        &[],
    )
    .await;
    let hex: String = SENTINEL.bytes().map(|b| format!("{b:02x}")).collect();
    for needle in [
        SENTINEL,
        "sentinel",
        "access_token",
        "refresh_token",
        hex.as_str(),
    ] {
        assert!(!text.contains(needle), "{needle} is in the table: {text}");
    }
    // nor in the raw bytes of any column
    for row in raw_rows(&raw).await {
        for bytes in [&row.5, &row.6] {
            assert!(!bytes.windows(8).any(|w| w == b"sentinel"));
        }
    }
    // nor in the sequence, the index or anywhere else the schema keeps a value: the only relations
    // are the table, its index and key, the sequence and the history
    let (relations,): (i64,) = fetch_one(
        &raw,
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'S')",
        &[],
    )
    .await;
    assert_eq!(
        relations, 3,
        "user_secrets, its sequence and the history table"
    );
}

#[tokio::test]
async fn versions_come_from_one_sequence_shared_by_every_replica() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };
    let alice = db.owner("alice");
    let bob = db.owner("bob");
    let other = db.another_store(ring_k1()).await;
    let mut seen = HashSet::new();
    for n in 0..10 {
        let server = credential(&format!("{n:010}"));
        // two replicas write different keys: no number is given twice in the store
        let v1 = store.put(&alice, &server, &material("a")).await.unwrap();
        let v2 = other
            .rotate(&alice, &server, v1, &material("b"))
            .await
            .unwrap();
        let v3 = store.put(&bob, &server, &material("c")).await.unwrap();
        for version in [v1, v2, v3] {
            assert!(seen.insert(version), "{version} was given twice");
        }
        assert!(v2 > v1);
    }
    let (last,): (i64,) = fetch_one(&raw, "SELECT last_value FROM user_secret_versions", &[]).await;
    assert_eq!(
        last,
        i64::try_from(seen.iter().map(|v| v.get()).max().unwrap()).unwrap()
    );
}

#[tokio::test]
async fn a_conflict_takes_no_version_and_changes_nothing() {
    let Some((db, store, raw)) = setup().await else {
        return;
    };

    let alice = db.owner("alice");
    let bob = db.owner("bob");
    let server = credential("0123456789");
    let v1 = store.put(&alice, &server, &material("one")).await.unwrap();
    let before = raw_rows(&raw).await;
    let (last_before,): (i64,) =
        fetch_one(&raw, "SELECT last_value FROM user_secret_versions", &[]).await;
    let stale = orch_ports::SecretVersion::new(v1.get() + 100);
    assert!(matches!(
        store
            .rotate(&alice, &server, stale, &material("lost"))
            .await,
        Err(SecretError::Conflict)
    ));
    assert!(matches!(
        store.rotate(&bob, &server, v1, &material("lost")).await,
        Err(SecretError::Conflict)
    ));
    let (last_after,): (i64,) =
        fetch_one(&raw, "SELECT last_value FROM user_secret_versions", &[]).await;
    assert_eq!(last_after, last_before, "a refused rotate takes no version");
    assert_eq!(raw_rows(&raw).await, before);
}

#[tokio::test]
async fn writers_of_one_key_land_in_version_order_whichever_call_they_make() {
    let Some((db, store, _raw)) = setup().await else {
        return;
    };
    let alice = db.owner("alice");
    let store = Arc::new(store);
    let other = Arc::new(db.another_store(ring_k1()).await);
    for round in 0..25_u32 {
        let server = credential(&format!("{round:010}"));
        let owner = alice.clone();
        let first = store
            .put(&owner, &server, &material("start"))
            .await
            .unwrap();
        let mut tasks = Vec::new();
        for n in 0..16_u32 {
            let (store, owner, server) = (
                (if n % 2 == 0 { &store } else { &other }).clone(),
                owner.clone(),
                server.clone(),
            );
            tasks.push(tokio::spawn(async move {
                let mine = material(&format!("writer {round}/{n}"));
                if n % 4 == 3 {
                    // a rotate from the first version: at most one wins, and only if no put got in first
                    store
                        .rotate(&owner, &server, first, &mine)
                        .await
                        .ok()
                        .map(|v| (v, n))
                } else {
                    Some((store.put(&owner, &server, &mine).await.unwrap(), n))
                }
            }));
        }
        let mut given = Vec::new();
        for task in tasks {
            if let Some(pair) = task.await.unwrap() {
                given.push(pair);
            }
        }
        let (held, got) = store.get(&owner, &server).await.unwrap().unwrap();
        let newest = given.iter().map(|(v, _)| *v).max().unwrap();
        assert_eq!(
            held, newest,
            "round {round}: the key holds the greatest version it gave"
        );
        let winner = given.iter().find(|(v, _)| *v == newest).unwrap().1;
        assert_eq!(
            got.expose(),
            format!("writer {round}/{winner}").as_bytes(),
            "round {round}"
        );
        let unique: HashSet<_> = given.iter().map(|(v, _)| *v).collect();
        assert_eq!(unique.len(), given.len(), "no version twice");
    }
}

#[tokio::test]
async fn the_migrations_are_idempotent_and_live_beside_the_orchestrators_own() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let alice = db.owner("alice");
    let pool = db.pool("orch-secrets-migrate", 8).await;
    let secrets = PgSecretStore::from_pool(pool.clone(), ring_k1());

    // both histories in one database and schema, run twice each, and at the same time
    secrets.migrate().await.unwrap();
    migrate_threads(&pool).await.unwrap();
    secrets.migrate().await.unwrap();
    migrate_threads(&pool).await.unwrap();
    let (a, b) = tokio::join!(secrets.migrate(), migrate_threads(&pool));
    a.unwrap();
    b.unwrap();

    let mine: Vec<(i64, String, bool)> = fetch_all(
        &pool,
        &format!("SELECT version, description, success FROM {MIGRATIONS_TABLE} ORDER BY version"),
        &[],
    )
    .await;
    assert_eq!(mine, [(1, "user secrets".to_owned(), true)]);
    // the orchestrator's history knows none of this crate's migrations, and this crate's none of its
    let theirs: Vec<(i64, String)> = fetch_all(
        &pool,
        "SELECT version, description FROM _sqlx_migrations ORDER BY version",
        &[],
    )
    .await;
    assert!(theirs.len() >= 20, "{theirs:?}");
    assert!(
        theirs
            .iter()
            .all(|(_, description)| description != "user secrets"),
        "{theirs:?}"
    );

    // the other way round, in a schema of its own, the secrets first
    let second = TestDb::new().await.unwrap();
    let pool = second.pool("orch-secrets-migrate-2", 4).await;
    let secrets = PgSecretStore::from_pool(pool.clone(), ring_k1());
    secrets.migrate().await.unwrap();
    migrate_threads(&pool).await.unwrap();
    secrets.migrate().await.unwrap();
    let (count,): (i64,) = fetch_one(
        &pool,
        &format!("SELECT count(*) FROM {MIGRATIONS_TABLE}"),
        &[],
    )
    .await;
    assert_eq!(count, 1);
    // and the store works
    let server = credential("0123456789");
    secrets.put(&alice, &server, &material("x")).await.unwrap();
}

#[tokio::test]
async fn a_history_that_does_not_match_is_corrupt_and_not_a_silent_success() {
    let Some((_db, store, raw)) = setup().await else {
        return;
    };
    execute(
        &raw,
        &format!("UPDATE {MIGRATIONS_TABLE} SET checksum = '\\x00'::bytea"),
        &[],
    )
    .await;
    let error = store.migrate().await.unwrap_err();
    assert!(matches!(error, SecretError::Corrupt(_)), "{error:?}");
}

#[tokio::test]
async fn a_database_that_is_gone_is_unavailable_whatever_the_call() {
    let Some((db, store, _raw)) = setup().await else {
        return;
    };

    let alice = db.owner("alice");
    let server = credential("0123456789");
    store.close().await;
    for error in [
        store.get(&alice, &server).await.map(|_| ()).unwrap_err(),
        store
            .put(&alice, &server, &material("x"))
            .await
            .map(|_| ())
            .unwrap_err(),
        store.delete(&alice, &server).await.unwrap_err(),
        store.migrate().await.unwrap_err(),
        store.rewrap().await.map(|_| ()).unwrap_err(),
    ] {
        assert!(
            matches!(error, SecretError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);
        assert!(is_outage(&error), "a database that is gone is an outage");
    }
}

/// A statement the database refuses is `Unavailable` (the port has no class for a bug) but is not an
/// outage: waiting does not cure it, and a caller that exits as for an outage would be restarted for
/// ever. A session that may write nothing stands for a role without the privilege `migrate` needs.
#[tokio::test]
async fn a_statement_the_database_refuses_is_unavailable_and_no_outage() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let read_only = db
        .pool_with(
            "orch-secrets-read-only",
            2,
            &[("default_transaction_read_only", "on")],
        )
        .await;
    let store = PgSecretStore::from_pool(read_only, ring_k1());

    let error = store.migrate().await.unwrap_err();
    assert!(
        matches!(error, SecretError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(error.class(), ErrorClass::Transient, "the port's class");
    assert!(
        !is_outage(&error),
        "the database refused CREATE TABLE and will again: {error:?}"
    );
    // and the source names the cause for whoever reads the log, with nothing of a secret
    assert!(
        std::error::Error::source(&error).is_some_and(|s| s.to_string().contains("read-only")),
        "{error:?}"
    );
}

/// The classification of `is_outage` is proven on a table of the driver's words (`src/store.rs`);
/// these are the driver's and the server's real errors, one of each way a call can fail.
#[tokio::test]
async fn real_errors_of_the_database_are_classified_as_the_table_says() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let alice = db.owner("alice");
    let server = credential("0123456789");

    // a server that refuses the connection: an outage
    let nowhere: orch_pg_connect::PgOptions = "postgres://127.0.0.1:1/none".parse().unwrap();
    let down = PgSecretStore::from_pool(
        nowhere
            .pool(1, std::time::Duration::from_millis(300))
            .unwrap(),
        ring_k1(),
    )
    .get(&alice, &server)
    .await
    .unwrap_err();
    assert!(matches!(down, SecretError::Unavailable { .. }), "{down:?}");
    assert!(is_outage(&down), "{down:?}");

    // a pool that is closed: an outage
    let store = db.store(ring_k1()).await;
    store.put(&alice, &server, &material("x")).await.unwrap();
    store.close().await;
    let closed = store.get(&alice, &server).await.map(|_| ()).unwrap_err();
    assert!(is_outage(&closed), "{closed:?}");

    // a row that does not decode (a column of another type than this crate wrote): Corrupt
    let raw = db.pool("orch-secrets-raw", 2).await;
    execute(
        &raw,
        "ALTER TABLE user_secrets ALTER COLUMN v TYPE integer",
        &[],
    )
    .await;
    let store = db.another_store(ring_k1()).await;
    let corrupt = store.get(&alice, &server).await.map(|_| ()).unwrap_err();
    assert!(matches!(corrupt, SecretError::Corrupt(_)), "{corrupt:?}");
    let report = store.rewrap().await.map(|_| ()).unwrap_err();
    assert!(matches!(report, SecretError::Corrupt(_)), "{report:?}");
}

/// A write the server refuses (SQLSTATE 25006, a read-only session): `Unavailable`, no outage,
/// and the source names the cause. A test of its own: through a pooler the read-only setting is
/// the database's (`TestDb::pool_with`), so nothing else may use that database after it.
#[tokio::test]
async fn a_write_the_server_refuses_is_unavailable_and_no_outage() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let alice = db.owner("alice");
    let server = credential("0123456789");
    PgSecretStore::migrate_direct(&db.pinned_direct_url())
        .await
        .unwrap();
    let read_only = db
        .pool_with(
            "orch-secrets-read-only-put",
            2,
            &[("default_transaction_read_only", "on")],
        )
        .await;
    let refused = PgSecretStore::from_pool(read_only, ring_k1())
        .put(&alice, &server, &material("x"))
        .await
        .unwrap_err();
    assert!(
        matches!(refused, SecretError::Unavailable { .. }),
        "{refused:?}"
    );
    assert!(!is_outage(&refused), "{refused:?}");
    assert!(
        std::error::Error::source(&refused)
            .is_some_and(|s| s.to_string().contains("read-only") && s.to_string().contains("25006")),
        "{refused:?}"
    );
}

/// A migration whose SQL the database refuses (`MigrateError::Execute`): here `user_secrets` was made
/// by hand before the first migration, which then fails with SQLSTATE `42P07`. Waiting does not
/// cure it, so it is `Unavailable` and no outage, and the source names the cause.
#[tokio::test]
async fn a_migration_the_database_refuses_is_unavailable_and_no_outage() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let raw = db.pool("orch-secrets-raw", 2).await;
    execute(&raw, "CREATE TABLE user_secrets (owner text)", &[]).await;
    let store = PgSecretStore::from_pool(db.pool("orch-secrets-refused", 2).await, ring_k1());
    let refused = [
        store.migrate().await.unwrap_err(),
        PgSecretStore::migrate_direct(&db.pinned_direct_url())
            .await
            .unwrap_err(),
    ];
    for error in refused {
        assert!(
            matches!(error, SecretError::Unavailable { .. }),
            "{error:?}"
        );
        assert!(!is_outage(&error), "{error:?}");
        assert!(
            std::error::Error::source(&error).is_some_and(|s| s.to_string().contains("42P07")),
            "{error:?}"
        );
    }
    // nothing of the migration is left: its transaction was rolled back
    let (sequences,): (i64,) = fetch_one(
        &raw,
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = current_schema() AND c.relkind = 'S'",
        &[],
    )
    .await;
    assert_eq!(sequences, 0);
}
