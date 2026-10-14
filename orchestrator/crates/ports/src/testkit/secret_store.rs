//! `SecretStore` conformance cases: what the credential broker relies on from any place that keeps
//! the opaque material of a person's credentials (ADR 0046, the amendment of 2026-10-11), stated
//! once and run against the in-memory store and every adapter (`orch-secrets-pg` and the later
//! ones).
//!
//! An implementation supplies a fresh store (`async fn() -> Option<S>`; `None` skips the suite).
//! Each case is `async fn(store)`, uses owners of its own (so a store shared by the cases of one
//! run is never confused) and gives up after 60 seconds.
//!
//! One case needs the adapter to **break its store** (`no_error_from_a_broken_store_shows_a_secret`):
//! the macro takes the means as a second argument, `breaker = path::to::async_fn`, an
//! `async fn(&S)` that makes every later call of that store fail as an unreachable store does (a
//! Postgres adapter closes its pool, the memory store is taken down). An adapter that cannot break
//! its store omits it, and the case is skipped with that reason printed.
//!
//! What is deliberately not asserted: how or where the material is kept or sealed, what a version
//! is beyond "a new one for every write, never given twice for a key, and increasing within a key",
//! and the wording of any error.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use orch_core::{Classify, ErrorClass, UserId};
use uuid::Uuid;

use crate::{
    MAX_SECRET_BYTES, SecretError, SecretMaterial, SecretServer, SecretStore, SecretVersion,
    UserToolServerId,
};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// A value no error and no `Debug` may show: planted in the material the secret cases keep.
const SENTINEL: &str = "sentinel-8c1f47a2-this-must-never-be-printed";

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A person no other case uses.
fn owner() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

/// The credential slot of the server `u-<suffix>`.
fn credential(suffix: &str) -> SecretServer {
    SecretServer::credential(&UserToolServerId::parse(&format!("u-{suffix}")).expect("an id"))
}

/// The pending-flow slot of the server `u-<suffix>`.
fn flow(suffix: &str) -> SecretServer {
    SecretServer::pending_flow(&UserToolServerId::parse(&format!("u-{suffix}")).expect("an id"))
}

fn material(bytes: &[u8]) -> SecretMaterial {
    SecretMaterial::new(bytes).expect("material within the limit")
}

/// `len` bytes that are not repeating (a xorshift sequence), so a store that drops, repeats or
/// reorders a piece does not give the same bytes back.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x.to_le_bytes()[3]
        })
        .collect()
}

async fn read<S: SecretStore>(
    store: &S,
    owner: &UserId,
    server: &SecretServer,
) -> Option<(SecretVersion, Vec<u8>)> {
    store
        .get(owner, server)
        .await
        .expect("get does not fail")
        .map(|(version, material)| (version, material.expose().to_vec()))
}

/// What was put is what comes back, with the version `put` returned.
pub async fn put_then_get_round_trips<S: SecretStore>(store: S) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let version = store
            .put(&owner, &server, &material(b"first"))
            .await
            .expect("put");
        let (got_version, got) = read(&store, &owner, &server).await.expect("it is kept");
        assert_eq!(got_version, version);
        assert_eq!(got, b"first");
    })
    .await;
}

/// A secret that is not there is `None`, not an error.
pub async fn a_missing_secret_is_none<S: SecretStore>(store: S) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        assert!(read(&store, &owner, &server).await.is_none());
    })
    .await;
}

/// The material comes back byte for byte: none at all, one byte, every byte value, a text with
/// multibyte characters and NULs, and the most there may be.
pub async fn material_reads_back_byte_for_byte<S: SecretStore>(store: S) {
    within(async {
        let owner = owner();
        let every_byte: Vec<u8> = (0..=255).collect();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("0000000001", Vec::new()),
            ("0000000002", vec![0]),
            ("0000000003", every_byte.clone()),
            ("0000000004", "tökén ✓ 日本語\0\r\n\"\\".as_bytes().to_vec()),
            ("0000000005", vec![0xff; 4096]),
            ("0000000006", noise(MAX_SECRET_BYTES, 7)),
            ("0000000007", noise(MAX_SECRET_BYTES / 2 + 1, 11)),
            ("0000000008", [every_byte.clone(), every_byte].concat()),
        ];
        for (suffix, bytes) in &cases {
            store
                .put(&owner, &credential(suffix), &material(bytes))
                .await
                .expect("put");
        }
        for (suffix, bytes) in &cases {
            let (_, got) = read(&store, &owner, &credential(suffix))
                .await
                .expect("it is kept");
            assert_eq!(&got, bytes, "{suffix}: {} bytes", bytes.len());
        }
    })
    .await;
}

/// `put` of a key that exists replaces the material and returns a version newer than the one it
/// replaced.
pub async fn put_replaces_and_gives_a_newer_version<S: SecretStore>(store: S) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let v1 = store.put(&owner, &server, &material(b"one")).await.unwrap();
        let v2 = store.put(&owner, &server, &material(b"two")).await.unwrap();
        assert!(v2 > v1, "{v2:?} should be newer than {v1:?}");
        let (version, got) = read(&store, &owner, &server).await.unwrap();
        assert_eq!((version, got.as_slice()), (v2, &b"two"[..]));
    })
    .await;
}

/// `put` of material that is the same as what is kept is still a write: it gives a new version, and
/// a caller that read the old one can no longer rotate from it.
pub async fn putting_identical_material_again_gives_a_new_version<S: SecretStore>(store: S) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let v1 = store
            .put(&owner, &server, &material(b"same"))
            .await
            .unwrap();
        let v2 = store
            .put(&owner, &server, &material(b"same"))
            .await
            .unwrap();
        assert_ne!(v1, v2, "a write is a new version, whatever it writes");
        assert!(v2 > v1, "{v2:?} should be newer than {v1:?}");
        assert_eq!(
            read(&store, &owner, &server).await.unwrap(),
            (v2, b"same".to_vec())
        );
        let error = store
            .rotate(&owner, &server, v1, &material(b"other"))
            .await
            .expect_err("the version read before the identical write");
        assert!(matches!(error, SecretError::Conflict), "{error:?}");
        // and so is a rotate that writes what is kept
        let v3 = store
            .rotate(&owner, &server, v2, &material(b"same"))
            .await
            .unwrap();
        assert_ne!(v2, v3);
    })
    .await;
}

/// `delete` removes the secret and only that one, and removing what is not there succeeds.
pub async fn delete_removes_the_secret_and_is_idempotent<S: SecretStore>(store: S) {
    within(async {
        let owner = owner();
        let (a, b) = (credential("0000000001"), credential("0000000002"));
        store.put(&owner, &a, &material(b"a")).await.unwrap();
        store.put(&owner, &b, &material(b"b")).await.unwrap();

        store.delete(&owner, &a).await.expect("delete");
        assert!(read(&store, &owner, &a).await.is_none());
        assert_eq!(read(&store, &owner, &b).await.unwrap().1, b"b");

        store.delete(&owner, &a).await.expect("the second delete");
        store
            .delete(&owner, &credential("0000000099"))
            .await
            .expect("delete of what never was");
        assert_eq!(read(&store, &owner, &b).await.unwrap().1, b"b");
    })
    .await;
}

/// `rotate` with the version that is stored replaces the material and returns a newer version; it
/// can be done again from the new one.
pub async fn rotate_with_the_stored_version_replaces<S: SecretStore>(store: S) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let v1 = store.put(&owner, &server, &material(b"one")).await.unwrap();
        let v2 = store
            .rotate(&owner, &server, v1, &material(b"two"))
            .await
            .expect("rotate from the stored version");
        assert!(v2 > v1);
        assert_eq!(
            read(&store, &owner, &server).await.unwrap(),
            (v2, b"two".to_vec())
        );
        let v3 = store
            .rotate(&owner, &server, v2, &material(b"three"))
            .await
            .expect("rotate again");
        assert!(v3 > v2);
        assert_eq!(
            read(&store, &owner, &server).await.unwrap(),
            (v3, b"three".to_vec())
        );
    })
    .await;
}

/// `rotate` with a version that is not the stored one is a `Conflict` (class `Conflict`) and
/// changes nothing: after a rotate, and after a `put` that replaced the secret.
pub async fn rotate_with_a_stale_version_is_a_conflict_and_changes_nothing<S: SecretStore>(
    store: S,
) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let v1 = store.put(&owner, &server, &material(b"one")).await.unwrap();
        let v2 = store
            .rotate(&owner, &server, v1, &material(b"two"))
            .await
            .unwrap();

        let error = store
            .rotate(&owner, &server, v1, &material(b"lost"))
            .await
            .expect_err("a stale version");
        assert!(matches!(error, SecretError::Conflict), "{error:?}");
        assert_eq!(error.class(), ErrorClass::Conflict);
        assert_eq!(
            read(&store, &owner, &server).await.unwrap(),
            (v2, b"two".to_vec())
        );

        // a put replaced it as well: the version read before is stale
        let v3 = store
            .put(&owner, &server, &material(b"three"))
            .await
            .unwrap();
        let error = store
            .rotate(&owner, &server, v2, &material(b"lost"))
            .await
            .expect_err("a version a put replaced");
        assert!(matches!(error, SecretError::Conflict), "{error:?}");
        assert_eq!(
            read(&store, &owner, &server).await.unwrap(),
            (v3, b"three".to_vec())
        );
    })
    .await;
}

/// `rotate` of a secret that is not there is a `Conflict` and creates nothing: the version a caller
/// read is gone.
pub async fn rotate_of_a_missing_secret_is_a_conflict<S: SecretStore>(store: S) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let error = store
            .rotate(&owner, &server, SecretVersion::new(1), &material(b"x"))
            .await
            .expect_err("nothing to rotate");
        assert!(matches!(error, SecretError::Conflict), "{error:?}");
        assert!(read(&store, &owner, &server).await.is_none());

        // and one that was deleted after it was read
        let v1 = store.put(&owner, &server, &material(b"one")).await.unwrap();
        store.delete(&owner, &server).await.unwrap();
        let error = store
            .rotate(&owner, &server, v1, &material(b"x"))
            .await
            .expect_err("deleted since it was read");
        assert!(matches!(error, SecretError::Conflict), "{error:?}");
        assert!(read(&store, &owner, &server).await.is_none());
    })
    .await;
}

/// Replicas that refresh at the same instant read one version and rotate from it: exactly one
/// wins, the others have a `Conflict`, and what is kept is the winner's (the version it was given
/// and its material, whole).
pub async fn concurrent_rotates_from_one_version_have_one_winner<S: SecretStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        let (owner, server) = (owner(), credential("0000000001"));
        let v1 = store.put(&owner, &server, &material(b"v1")).await.unwrap();

        let tasks: Vec<_> = (0..8_u8)
            .map(|n| {
                let (store, owner, server) = (Arc::clone(&store), owner.clone(), server.clone());
                tokio::spawn(async move {
                    let mine = vec![n; 64];
                    let result = store.rotate(&owner, &server, v1, &material(&mine)).await;
                    (mine, result)
                })
            })
            .collect();
        let mut winners = Vec::new();
        for task in tasks {
            let (mine, result) = task.await.expect("the task ran");
            match result {
                Ok(version) => winners.push((version, mine)),
                Err(SecretError::Conflict) => {}
                Err(other) => panic!("neither rotated nor a conflict: {other}"),
            }
        }
        assert_eq!(winners.len(), 1, "exactly one rotate wins");
        assert_eq!(
            read(store.as_ref(), &owner, &server).await.unwrap(),
            winners[0]
        );
    })
    .await;
}

/// Writers that race with plain `put`s all succeed and leave one whole secret: a version a writer
/// was given, with that writer's material and no other's, and **the greatest version any writer was
/// given**: the adapter serializes the writers of one key, so the write that landed last holds the
/// version that was numbered last.
pub async fn concurrent_puts_of_one_key_leave_one_whole_secret<S: SecretStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        let (owner, server) = (owner(), credential("0000000001"));
        let tasks: Vec<_> = (0..8_u8)
            .map(|n| {
                let (store, owner, server) = (Arc::clone(&store), owner.clone(), server.clone());
                tokio::spawn(async move {
                    let mine = noise(2048, u64::from(n) + 100);
                    let version = store
                        .put(&owner, &server, &material(&mine))
                        .await
                        .expect("put");
                    (version, mine)
                })
            })
            .collect();
        let mut written = Vec::new();
        for task in tasks {
            written.push(task.await.expect("the task ran"));
        }
        let kept = read(store.as_ref(), &owner, &server).await.unwrap();
        assert!(
            written.contains(&kept),
            "what is kept is one writer's version with that writer's material"
        );
        let newest = written.iter().map(|(version, _)| *version).max().unwrap();
        assert_eq!(
            kept.0, newest,
            "what is kept holds the greatest version any writer was given"
        );
        let mut versions: Vec<_> = written.iter().map(|(version, _)| *version).collect();
        versions.sort_unstable();
        versions.dedup();
        assert_eq!(versions.len(), written.len(), "no version was given twice");
    })
    .await;
}

/// A version is not handed out twice for one key, even across a `delete`: a caller that read
/// version 1, then saw the secret deleted and made again, cannot rotate from what it read.
pub async fn a_version_is_never_reused_after_a_delete<S: SecretStore>(store: S) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let v1 = store
            .put(&owner, &server, &material(b"old grant"))
            .await
            .unwrap();
        store.delete(&owner, &server).await.unwrap();
        let v2 = store
            .put(&owner, &server, &material(b"new grant"))
            .await
            .unwrap();
        assert_ne!(
            v2, v1,
            "a version read before the delete never matches again"
        );

        // the refresh of the old grant must not overwrite the new one
        let error = store
            .rotate(&owner, &server, v1, &material(b"refreshed old grant"))
            .await
            .expect_err("the version read before the delete");
        assert!(matches!(error, SecretError::Conflict), "{error:?}");
        assert_eq!(
            read(&store, &owner, &server).await.unwrap(),
            (v2, b"new grant".to_vec())
        );
    })
    .await;
}

/// A second owner never sees, changes or removes the first's material, with the same server id.
pub async fn owners_never_see_each_others_material<S: SecretStore>(store: S) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let server = credential("0000000001");
        let va = store
            .put(&alice, &server, &material(b"alice's token"))
            .await
            .unwrap();

        // bob reads nothing, and cannot rotate from alice's version
        assert!(read(&store, &bob, &server).await.is_none());
        let error = store
            .rotate(&bob, &server, va, &material(b"bob's"))
            .await
            .expect_err("bob has no such secret");
        assert!(matches!(error, SecretError::Conflict), "{error:?}");
        assert!(read(&store, &bob, &server).await.is_none());

        // bob keeps his own under the same server id, and the two never meet
        let vb = store
            .put(&bob, &server, &material(b"bob's token"))
            .await
            .unwrap();
        assert_eq!(
            read(&store, &alice, &server).await.unwrap(),
            (va, b"alice's token".to_vec())
        );
        assert_eq!(
            read(&store, &bob, &server).await.unwrap(),
            (vb, b"bob's token".to_vec())
        );

        // and removing bob's leaves alice's
        store.delete(&bob, &server).await.unwrap();
        assert!(read(&store, &bob, &server).await.is_none());
        assert_eq!(
            read(&store, &alice, &server).await.unwrap().1,
            b"alice's token"
        );
    })
    .await;
}

/// The credential of a server, the state of its pending flow and another server's credential are
/// three secrets of their own.
pub async fn a_server_and_its_pending_flow_are_separate_secrets<S: SecretStore>(store: S) {
    within(async {
        let owner = owner();
        let (token, pending, other) = (
            credential("0000000001"),
            flow("0000000001"),
            credential("0000000002"),
        );
        store
            .put(&owner, &token, &material(b"token"))
            .await
            .unwrap();
        store
            .put(&owner, &pending, &material(b"verifier"))
            .await
            .unwrap();
        store
            .put(&owner, &other, &material(b"other"))
            .await
            .unwrap();

        assert_eq!(read(&store, &owner, &token).await.unwrap().1, b"token");
        assert_eq!(read(&store, &owner, &pending).await.unwrap().1, b"verifier");
        assert_eq!(read(&store, &owner, &other).await.unwrap().1, b"other");

        store.delete(&owner, &pending).await.unwrap();
        assert!(read(&store, &owner, &pending).await.is_none());
        assert_eq!(read(&store, &owner, &token).await.unwrap().1, b"token");
        assert_eq!(read(&store, &owner, &other).await.unwrap().1, b"other");
    })
    .await;
}

/// Everything a case can make a store say, flattened to text: the error, its `Debug` and every
/// source of it.
fn everything_said_by(error: &SecretError) -> String {
    let mut said = format!("{error} {error:?}");
    let mut source = std::error::Error::source(error);
    while let Some(e) = source {
        said.push_str(&format!(" {e} {e:?}"));
        source = e.source();
    }
    said
}

/// No `Debug` of the material or of what a call returned shows the material, and the errors a
/// caller can provoke (a stale version, a missing secret) do not either: the sentinel planted in the
/// material is nowhere, neither as it is nor in hex.
///
/// Those errors are `Conflict`, which holds nothing. The errors that can hold text, the ones of a
/// store that fails, are the case below.
pub async fn no_error_or_debug_text_shows_a_secret<S: SecretStore>(store: S) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let planted = material(format!("{{\"access_token\":\"{SENTINEL}\"}}").as_bytes());
        let mut said = vec![format!("{planted:?}"), format!("{owner:?} {server:?}")];

        let put = store.put(&owner, &server, &planted).await;
        said.push(format!("{put:?}"));
        let version = put.expect("put");
        said.push(format!("{:?}", store.get(&owner, &server).await));

        let rotated = store.rotate(&owner, &server, version, &planted).await;
        said.push(format!("{rotated:?}"));
        let newer = rotated.expect("rotate");
        // the errors a store can be made to give by a caller: a stale version, a missing secret
        for error in [
            store
                .rotate(&owner, &server, version, &planted)
                .await
                .expect_err("stale"),
            store
                .rotate(&owner, &credential("0000000002"), newer, &planted)
                .await
                .expect_err("missing"),
        ] {
            said.push(everything_said_by(&error));
        }
        said.push(format!("{:?}", store.delete(&owner, &server).await));
        said.push(format!("{:?}", store.get(&owner, &server).await));

        let hex: String = SENTINEL.bytes().map(|b| format!("{b:02x}")).collect();
        for text in said {
            assert!(!text.contains(SENTINEL), "the secret is in: {text}");
            assert!(!text.contains(&hex), "the secret, in hex, is in: {text}");
        }
    })
    .await;
}

/// A store that is broken fails every call with an error that shows nothing of the material that was
/// in flight: `put`, `rotate` (the material is in the call), `get` and `delete` (it is in the store)
/// with a sentinel in it. The sentinel is looked for, as it is and in hex, in the error's `Display`,
/// in its `Debug` and in every error of its `source` chain.
///
/// `break_store` makes the store fail as an unreachable one does (see the module's docs).
pub async fn no_error_from_a_broken_store_shows_a_secret<S: SecretStore>(
    store: S,
    break_store: impl AsyncFnOnce(&S),
) {
    within(async {
        let (owner, server) = (owner(), credential("0000000001"));
        let planted = material(format!("{{\"refresh_token\":\"{SENTINEL}\"}}").as_bytes());
        let version = store
            .put(&owner, &server, &planted)
            .await
            .expect("put, while the store is whole");

        break_store(&store).await;

        let errors = [
            store
                .put(&owner, &server, &planted)
                .await
                .expect_err("a broken store cannot put"),
            store
                .rotate(&owner, &server, version, &planted)
                .await
                .expect_err("a broken store cannot rotate"),
            store
                .get(&owner, &server)
                .await
                .expect_err("a broken store cannot get"),
            store
                .delete(&owner, &server)
                .await
                .expect_err("a broken store cannot delete"),
        ];
        let hex: String = SENTINEL.bytes().map(|b| format!("{b:02x}")).collect();
        for error in &errors {
            assert!(
                matches!(error, SecretError::Unavailable { .. }),
                "a broken store is unavailable: {error:?}"
            );
            assert_eq!(error.class(), ErrorClass::Transient);
            let said = everything_said_by(error);
            assert!(!said.contains(SENTINEL), "the secret is in: {said}");
            assert!(!said.contains(&hex), "the secret, in hex, is in: {said}");
        }
    })
    .await;
}
