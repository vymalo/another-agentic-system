//! `rewrap`: key rotation by key id. Skipped unless `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::UserId;
use orch_ports::{SecretError, SecretMaterial, SecretServer, SecretStore as _, UserToolServerId};
use orch_secrets_pg::{
    PgSecretStore, RewrapFailure, RewrapFailureKind, RewrapReport, advisory_lock_key,
};
use sqlx::PgPool;
use support::{TestDb, hold_lock, ring, wait_for_a_waiter};

/// The `n`th person of this test: no other test has them (see [`TestDb::owner`]), so no lock of
/// their keys is anyone else's.
fn owner(db: &TestDb, n: u32) -> UserId {
    db.owner(&format!("user{n}"))
}

fn credential(n: u32) -> SecretServer {
    SecretServer::credential(&UserToolServerId::parse(&format!("u-{n:010}")).unwrap())
}

fn flow(n: u32) -> SecretServer {
    SecretServer::pending_flow(&UserToolServerId::parse(&format!("u-{n:010}")).unwrap())
}

fn material(text: &str) -> SecretMaterial {
    SecretMaterial::new(text.as_bytes()).unwrap()
}

type RawRow = (String, String, i64, i16, String, Vec<u8>, Vec<u8>);

/// The columns of a row without its key: version, format, key id, nonce, ciphertext.
type Sealed = (i64, i16, String, Vec<u8>, Vec<u8>);

async fn raw_rows(pool: &PgPool) -> Vec<RawRow> {
    sqlx::query_as(
        "SELECT owner, server, version, v, kid, nonce, ciphertext FROM user_secrets ORDER BY owner, server",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A database with: `old`, a deployment on key `k1`; `new`, one that made `k2` current and still
/// holds `k1` (the middle of a rotation); and a raw pool.
struct World {
    db: TestDb,
    old: PgSecretStore,
    new: PgSecretStore,
    raw: PgPool,
}

async fn world() -> Option<World> {
    let db = TestDb::new().await?;
    let old = db.store(ring("k1", &[("k1", 1)])).await;
    let new = db.another_store(ring("k2", &[("k1", 1), ("k2", 2)])).await;
    let raw = db.pool("orch-secrets-raw", 4).await;
    Some(World { db, old, new, raw })
}

#[tokio::test]
async fn an_empty_store_has_nothing_to_rewrap() {
    let Some(w) = world().await else { return };
    let report = w.new.rewrap().await.unwrap();
    assert_eq!(report, RewrapReport::default());
    assert!(report.is_complete());
}

#[tokio::test]
async fn rewrap_moves_every_row_to_the_current_key_and_keeps_versions_and_material() {
    let Some(w) = world().await else { return };
    let mut kept = Vec::new();
    for n in 0..5 {
        for server in [credential(n), flow(n)] {
            let owner = owner(&w.db, n % 2);
            let text = format!("material of {owner} {server}");
            let version = w.old.put(&owner, &server, &material(&text)).await.unwrap();
            kept.push((owner, server, version, text));
        }
    }
    let before = raw_rows(&w.raw).await;
    let updated_before: Vec<(String,)> =
        sqlx::query_as("SELECT updated::text FROM user_secrets ORDER BY owner, server")
            .fetch_all(&w.raw)
            .await
            .unwrap();
    assert!(before.iter().all(|row| row.4 == "k1"));

    let report = w.new.rewrap().await.unwrap();
    assert_eq!(
        report,
        RewrapReport {
            rewrapped: 10,
            already_current: 0,
            changed: 0,
            failed: 0,
            failures: vec![]
        }
    );
    assert!(report.is_complete());

    let after = raw_rows(&w.raw).await;
    assert_eq!(after.len(), before.len());
    for (was, is) in before.iter().zip(&after) {
        assert_eq!(
            (&was.0, &was.1, was.2),
            (&is.0, &is.1, is.2),
            "owner, server and version stay"
        );
        assert_eq!(is.4, "k2");
        assert_ne!(was.5, is.5, "a new nonce");
        assert_ne!(was.6, is.6, "a new ciphertext");
    }
    // `updated` says when the material last changed, and it did not
    let updated_after: Vec<(String,)> =
        sqlx::query_as("SELECT updated::text FROM user_secrets ORDER BY owner, server")
            .fetch_all(&w.raw)
            .await
            .unwrap();
    assert_eq!(updated_after, updated_before);

    // a deployment that has dropped k1 reads all of it, at the versions it had
    let only_new = w.db.another_store(ring("k2", &[("k2", 2)])).await;
    for (owner, server, version, text) in &kept {
        let (got_version, got) = only_new.get(owner, server).await.unwrap().unwrap();
        assert_eq!((got_version, got.expose()), (*version, text.as_bytes()));
        // and the same through the store that rewrapped
        assert_eq!(w.new.get(owner, server).await.unwrap().unwrap().0, *version);
    }

    // a second run finds nothing to do
    let again = w.new.rewrap().await.unwrap();
    assert_eq!(
        again,
        RewrapReport {
            rewrapped: 0,
            already_current: 10,
            changed: 0,
            failed: 0,
            failures: vec![]
        }
    );
    assert_eq!(raw_rows(&w.raw).await, after, "a second run writes nothing");
}

#[tokio::test]
async fn rewrap_goes_through_more_rows_than_a_page() {
    let Some(w) = world().await else { return };
    let total = 230_u32;
    let mut tasks = Vec::new();
    for lane in 0..10_u32 {
        let (old, db) = (w.old.clone(), w.db.clone());
        tasks.push(tokio::spawn(async move {
            for n in (lane..total).step_by(10) {
                old.put(
                    &owner(&db, n),
                    &credential(n),
                    &material(&format!("secret {n}")),
                )
                .await
                .unwrap();
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let report = w.new.rewrap().await.unwrap();
    assert_eq!(
        (report.rewrapped, report.failed),
        (u64::from(total), 0),
        "{report:?}"
    );
    let (under_k1,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_secrets WHERE kid <> 'k2'")
        .fetch_one(&w.raw)
        .await
        .unwrap();
    assert_eq!(under_k1, 0);
}

#[tokio::test]
async fn a_row_under_a_key_that_is_not_held_is_counted_as_failed_and_the_rest_are_done() {
    let Some(w) = world().await else { return };
    // k0 was removed from the file before the rows under it were re-sealed
    let lost = w.db.another_store(ring("k0", &[("k0", 9)])).await;
    lost.put(&owner(&w.db, 1), &credential(1), &material("under k0"))
        .await
        .unwrap();
    lost.put(&owner(&w.db, 1), &flow(1), &material("also under k0"))
        .await
        .unwrap();
    w.old
        .put(&owner(&w.db, 2), &credential(2), &material("under k1"))
        .await
        .unwrap();
    w.new
        .put(
            &owner(&w.db, 3),
            &credential(3),
            &material("under k2 already"),
        )
        .await
        .unwrap();
    let before_lost: Vec<RawRow> = raw_rows(&w.raw)
        .await
        .into_iter()
        .filter(|row| row.4 == "k0")
        .collect();
    assert_eq!(before_lost.len(), 2);

    let report = w.new.rewrap().await.unwrap();
    assert_eq!(
        (
            report.rewrapped,
            report.already_current,
            report.changed,
            report.failed
        ),
        (1, 1, 0, 2)
    );
    assert!(!report.is_complete());
    let mut failures = report.failures.clone();
    failures.sort_by(|a, b| a.server.cmp(&b.server));
    assert_eq!(
        failures,
        [
            RewrapFailure {
                server: credential(1).to_string(),
                kid: "k0".into(),
                kind: RewrapFailureKind::KeyUnknown
            },
            RewrapFailure {
                server: flow(1).to_string(),
                kid: "k0".into(),
                kind: RewrapFailureKind::KeyUnknown
            },
        ]
    );
    // the rows it could not open are exactly as they were
    let after_lost: Vec<RawRow> = raw_rows(&w.raw)
        .await
        .into_iter()
        .filter(|row| row.4 == "k0")
        .collect();
    assert_eq!(after_lost, before_lost);
    // and the rest are under k2
    assert_eq!(
        w.new
            .get(&owner(&w.db, 2), &credential(2))
            .await
            .unwrap()
            .unwrap()
            .1
            .expose(),
        b"under k1"
    );
    assert!(matches!(
        w.new.get(&owner(&w.db, 1), &credential(1)).await,
        Err(SecretError::KeyUnknown)
    ));

    // the old key put back in the file, the same run finishes the job
    let back =
        w.db.another_store(ring("k2", &[("k0", 9), ("k1", 1), ("k2", 2)]))
            .await;
    let report = back.rewrap().await.unwrap();
    assert_eq!(
        (report.rewrapped, report.already_current, report.failed),
        (2, 2, 0)
    );
    assert!(report.is_complete());
}

#[tokio::test]
async fn a_row_that_does_not_open_is_counted_as_failed_and_left_alone() {
    let Some(w) = world().await else { return };
    w.old
        .put(&owner(&w.db, 1), &credential(1), &material("good"))
        .await
        .unwrap();
    w.old
        .put(&owner(&w.db, 1), &credential(2), &material("bad"))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE user_secrets SET ciphertext = set_byte(ciphertext, 0, get_byte(ciphertext, 0) # 1) \
         WHERE server = $1",
    )
    .bind(credential(2).to_string())
    .execute(&w.raw)
    .await
    .unwrap();
    let bad_before: Vec<RawRow> = raw_rows(&w.raw)
        .await
        .into_iter()
        .filter(|row| row.1 == credential(2).to_string())
        .collect();

    let report = w.new.rewrap().await.unwrap();
    assert_eq!((report.rewrapped, report.failed), (1, 1), "{report:?}");
    assert_eq!(
        report.failures,
        [RewrapFailure {
            server: credential(2).to_string(),
            kid: "k1".into(),
            kind: RewrapFailureKind::Corrupt
        }]
    );
    let bad_after: Vec<RawRow> = raw_rows(&w.raw)
        .await
        .into_iter()
        .filter(|row| row.1 == credential(2).to_string())
        .collect();
    assert_eq!(bad_after, bad_before);
    let said = format!("{report:?}");
    assert!(
        !said.contains("example.test"),
        "the owner is not named: {said}"
    );
}

/// A key id names bytes, and the id is all a row says of them. If the bytes behind an id change (the
/// property of the secret store behind `k1` is overwritten; `k1` is removed and a different key is
/// added under the name), every row sealed under it is unreadable, and a run that counted the rows
/// under the current key by their id would report them as current and exit 0.
#[tokio::test]
async fn a_row_under_the_current_key_is_opened_and_counted_as_failed_when_it_does_not() {
    let Some(w) = world().await else { return };
    for n in 0..5 {
        w.old
            .put(&owner(&w.db, n), &credential(n), &material("x"))
            .await
            .unwrap();
    }
    let before = raw_rows(&w.raw).await;

    // the deployment's `k1` is other bytes than the ones that sealed these rows, and `k1` is current
    let changed = w.db.another_store(ring("k1", &[("k1", 9)])).await;
    let report = changed.rewrap().await.unwrap();
    assert_eq!(
        (
            report.rewrapped,
            report.already_current,
            report.changed,
            report.failed
        ),
        (0, 0, 0, 5),
        "{report:?}"
    );
    assert!(!report.is_complete());
    assert!(
        report
            .failures
            .iter()
            .all(|f| f.kid == "k1" && f.kind == RewrapFailureKind::Corrupt),
        "{report:?}"
    );
    assert_eq!(raw_rows(&w.raw).await, before, "nothing was written");

    // the same when the id is not current: `k1` is back as other bytes, and `k2` seals
    let moved =
        w.db.another_store(ring("k2", &[("k1", 9), ("k2", 2)]))
            .await;
    let report = moved.rewrap().await.unwrap();
    assert_eq!((report.rewrapped, report.failed), (0, 5), "{report:?}");
    assert_eq!(raw_rows(&w.raw).await, before, "nothing was written");

    // with the bytes that sealed them they open, and are counted as current
    let report = w.old.rewrap().await.unwrap();
    assert_eq!(
        (report.rewrapped, report.already_current, report.failed),
        (0, 5, 0),
        "{report:?}"
    );
    assert!(report.is_complete());
}

/// Why `rewrap` keeps the version. A refresh claims the grant with `rotate(read)` and ends it with
/// `rotate(claim)`; a rewrap in between that made a new version would make the second a `Conflict`,
/// which the broker reads as "another replica took over" and so drops the refresh token the
/// authorization server has just rotated to.
#[tokio::test]
async fn a_refresh_that_is_claimed_before_a_rewrap_can_still_keep_its_tokens_after_it() {
    let Some(w) = world().await else { return };
    let (alice, server) = (owner(&w.db, 1), credential(1));
    let read = w
        .old
        .put(&alice, &server, &material("tokens, refresh token R1"))
        .await
        .unwrap();
    // the replica claims the refresh
    let claim = w
        .old
        .rotate(&alice, &server, read, &material("tokens, refreshing"))
        .await
        .unwrap();

    // the rewrap runs while the authorization server is being asked
    let report = w.new.rewrap().await.unwrap();
    assert_eq!(report.rewrapped, 1);
    assert_eq!(
        w.new.get(&alice, &server).await.unwrap().unwrap().0,
        claim,
        "the claim's version stands"
    );

    // the answer arrives: the refresh token the server rotated to is kept
    let kept = w
        .new
        .rotate(
            &alice,
            &server,
            claim,
            &material("tokens, refresh token R2"),
        )
        .await
        .expect("the version the claim made is still the stored one");
    assert!(kept > claim);
    assert_eq!(
        w.new
            .get(&alice, &server)
            .await
            .unwrap()
            .unwrap()
            .1
            .expose(),
        b"tokens, refresh token R2"
    );
}

#[tokio::test]
async fn a_rewrap_racing_a_rotate_loses_nothing() {
    let Some(w) = world().await else { return };
    for round in 0..40_u32 {
        let (alice, server) = (owner(&w.db, round), credential(round));
        let read = w
            .old
            .put(&alice, &server, &material("before"))
            .await
            .unwrap();
        let new = w.new.clone();
        let rotate = {
            let (new, alice, server) = (new.clone(), alice.clone(), server.clone());
            tokio::spawn(async move {
                if round % 2 == 0 {
                    tokio::task::yield_now().await;
                }
                new.rotate(&alice, &server, read, &material(&format!("after {round}")))
                    .await
            })
        };
        let rewrap = tokio::spawn(async move { new.rewrap().await });
        let version = rotate
            .await
            .unwrap()
            .expect("a rewrap never makes a rotate a conflict");
        let report = rewrap.await.unwrap().unwrap();
        assert_eq!(report.failed, 0, "round {round}: {report:?}");
        let (held, got) = w.new.get(&alice, &server).await.unwrap().unwrap();
        assert_eq!(held, version, "round {round}");
        assert_eq!(
            got.expose(),
            format!("after {round}").as_bytes(),
            "round {round}"
        );
    }
    // whichever way each race went, the rows are under the key that seals now
    let report = w.new.rewrap().await.unwrap();
    assert_eq!((report.failed, report.changed), (0, 0), "{report:?}");
    let (others,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_secrets WHERE kid <> 'k2'")
        .fetch_one(&w.raw)
        .await
        .unwrap();
    assert_eq!(others, 0);
}

#[tokio::test]
async fn a_rewrap_beside_writers_and_deleters_fails_nothing_and_corrupts_nothing() {
    let Some(w) = world().await else { return };
    for n in 0..30 {
        w.old
            .put(
                &owner(&w.db, n),
                &credential(n),
                &material(&format!("start {n}")),
            )
            .await
            .unwrap();
    }
    let mut tasks = Vec::new();
    for lane in 0..3_u32 {
        let (new, db) = (w.new.clone(), w.db.clone());
        tasks.push(tokio::spawn(async move {
            for n in (lane..30).step_by(3) {
                match n % 3 {
                    0 => {
                        new.put(
                            &owner(&db, n),
                            &credential(n),
                            &material(&format!("put {n}")),
                        )
                        .await
                        .unwrap();
                    }
                    1 => new.delete(&owner(&db, n), &credential(n)).await.unwrap(),
                    _ => {
                        if let Some((version, _)) =
                            new.get(&owner(&db, n), &credential(n)).await.unwrap()
                        {
                            let _ = new
                                .rotate(
                                    &owner(&db, n),
                                    &credential(n),
                                    version,
                                    &material(&format!("rotated {n}")),
                                )
                                .await;
                        }
                    }
                }
            }
        }));
    }
    let rewraps =
        [w.new.clone(), w.new.clone()].map(|new| tokio::spawn(async move { new.rewrap().await }));
    for task in tasks {
        task.await.unwrap();
    }
    for rewrap in rewraps {
        let report = rewrap.await.unwrap().unwrap();
        assert_eq!(report.failed, 0, "{report:?}");
    }
    // a last run settles whatever the first ones saw change, and every row that is left opens
    let report = w.new.rewrap().await.unwrap();
    assert_eq!((report.failed, report.changed), (0, 0), "{report:?}");
    for (owner_text, server_text, _, _, kid, _, _) in raw_rows(&w.raw).await {
        assert_eq!(kid, "k2");
        let server = SecretServer::parse(&server_text).unwrap();
        assert!(
            w.new
                .get(&UserId::new(&owner_text), &server)
                .await
                .unwrap()
                .is_some()
        );
    }
}

#[tokio::test]
async fn two_rewraps_at_once_do_each_row_once() {
    let Some(w) = world().await else { return };
    for n in 0..60 {
        w.old
            .put(&owner(&w.db, n), &credential(n), &material("x"))
            .await
            .unwrap();
    }
    let (a, b) = (
        w.new.clone(),
        w.db.another_store(ring("k2", &[("k1", 1), ("k2", 2)]))
            .await,
    );
    let (a, b) = tokio::join!(
        tokio::spawn(async move { a.rewrap().await }),
        tokio::spawn(async move { b.rewrap().await })
    );
    let (a, b) = (a.unwrap().unwrap(), b.unwrap().unwrap());
    assert_eq!(a.rewrapped + b.rewrapped, 60, "{a:?} {b:?}");
    assert_eq!((a.failed, b.failed), (0, 0));
    // what one did, the other counted as current, or saw while it was done
    assert_eq!(a.rewrapped + a.already_current + a.changed, 60);
    assert_eq!(b.rewrapped + b.already_current + b.changed, 60);
}

/// The row the run saw is not the row it finds when it has the key's lock: it is left alone and
/// counted, whether another writer changed it or deleted it.
#[tokio::test]
async fn a_row_that_changes_while_the_run_waits_for_its_lock_is_left_alone_and_counted() {
    for deleted in [false, true] {
        let Some(w) = world().await else { return };
        let (alice, server) = (owner(&w.db, 1), credential(1));
        w.old.put(&alice, &server, &material("x")).await.unwrap();

        // another writer is in the middle of its write: it holds the key's lock
        let lock = advisory_lock_key(&alice, &server);
        let mut writer = hold_lock(&w.raw, lock).await;
        let new = w.new.clone();
        let run = tokio::spawn(async move { new.rewrap().await });
        // the run has read the row and waits for this key's lock (the owner is this test's, so
        // nobody else's waiting can be mistaken for it)
        wait_for_a_waiter(&w.raw, lock).await;

        if deleted {
            sqlx::query("DELETE FROM user_secrets")
                .execute(&mut *writer)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE user_secrets SET version = version + 1000")
                .execute(&mut *writer)
                .await
                .unwrap();
        }
        let before = {
            // read through the writer's own session: the change is not committed yet
            sqlx::query_as::<_, Sealed>(
                "SELECT version, v, kid, nonce, ciphertext FROM user_secrets",
            )
            .fetch_all(&mut *writer)
            .await
            .unwrap()
        };
        writer.commit().await.unwrap();

        let report = run.await.unwrap().unwrap();
        assert_eq!(
            (report.rewrapped, report.changed, report.failed),
            (0, 1, 0),
            "deleted: {deleted}, {report:?}"
        );
        assert!(
            !report.is_complete(),
            "a run that saw a row change is not proof"
        );
        let after: Vec<Sealed> =
            sqlx::query_as("SELECT version, v, kid, nonce, ciphertext FROM user_secrets")
                .fetch_all(&w.raw)
                .await
                .unwrap();
        assert_eq!(after, before, "nothing was written by the run");
    }
}
