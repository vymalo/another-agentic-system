//! `VaultStore` conformance cases: what a configuration that names `vault:<name>#v<n>` relies on
//! from any place that keeps a person's named secrets (ADR 0083, amended 2026-11-21; decision row
//! 114 of 2026-11-22), stated once and run against the in-memory vault and every adapter
//! (`orch-secrets-pg` and the later ones).
//!
//! An implementation supplies a fresh store **and the clock its grace runs on** (`async fn() ->
//! Option<(S, FixedClock)>`; `None` skips the suite). The clock is the fixture's, not the case's:
//! the 24-hour rule is proved by moving it, never by a sleep, so a case takes a second at most.
//!
//! Each case uses owners and names of its own (so a store shared by the cases of one run is never
//! confused) and gives up after 60 seconds.
//!
//! What is deliberately not asserted: how or where the material is kept or sealed, how a store
//! numbers versions beyond "one more than the greatest, never given twice", and the wording of any
//! error.

use std::future::Future;
use std::time::Duration;

use orch_core::{Classify, ErrorClass, UserId};
use uuid::Uuid;

use crate::memory::FixedClock;
use crate::{
    MAX_SECRET_BYTES, SUPERSEDED_GRACE, SecretMaterial, SecretVersion, VaultError, VaultName,
    VaultRef, VaultStore,
};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// A value no error and no `Debug` may show: planted in the material the cases keep.
const SENTINEL: &str = "sentinel-2b7e51d4-this-must-never-be-printed";

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A person no other case uses.
fn owner() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

/// A name no other case uses.
fn name(hint: &str) -> VaultName {
    VaultName::parse(&format!(
        "{hint}-{}",
        &Uuid::now_v7().simple().to_string()[..12]
    ))
    .expect("a name")
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

/// The grace as a `std::time::Duration`, for the clock.
fn grace() -> Duration {
    Duration::from_secs(
        u64::try_from(SUPERSEDED_GRACE.as_secs()).expect("a grace of positive seconds"),
    )
}

/// What was written is what the reference reads, and a name's first version is 1.
pub async fn put_then_get_round_trips<S: VaultStore>(store: S, _clock: FixedClock) {
    within(async {
        let (owner, name) = (owner(), name("first"));
        let reference = store
            .put(&owner, &name, &material(b"first"))
            .await
            .expect("put");
        assert_eq!(
            reference,
            VaultRef::new(name.clone(), SecretVersion::new(1))
        );
        assert_eq!(reference.to_string(), format!("vault:{name}#v1"));
        assert_eq!(
            reference,
            VaultRef::parse(&reference.to_string()).expect("the reference reads back")
        );
        let got = store.get(&owner, &reference).await.expect("get");
        assert_eq!(got.expose(), b"first");
    })
    .await;
}

/// A second write of a name is `#v2`, a third `#v3`: one more than the greatest, and the version
/// that was current before each write is superseded (and still reads, within the grace).
pub async fn a_name_is_written_at_v1_and_the_next_write_at_v2<S: VaultStore>(
    store: S,
    _clock: FixedClock,
) {
    within(async {
        let (owner, rotated) = (owner(), name("rotation"));
        let first = store
            .put(&owner, &rotated, &material(b"one"))
            .await
            .unwrap();
        let second = store
            .put(&owner, &rotated, &material(b"two"))
            .await
            .unwrap();
        let third = store
            .put(&owner, &rotated, &material(b"three"))
            .await
            .unwrap();
        assert_eq!(first.version(), SecretVersion::new(1));
        assert_eq!(second.version(), SecretVersion::new(2));
        assert_eq!(third.version(), SecretVersion::new(3));
        assert_eq!(store.get(&owner, &first).await.unwrap().expose(), b"one");
        assert_eq!(store.get(&owner, &second).await.unwrap().expose(), b"two");
        assert_eq!(store.get(&owner, &third).await.unwrap().expose(), b"three");
        // another name starts at 1 of its own
        let other = store
            .put(&owner, &name("another"), &material(b"other"))
            .await
            .unwrap();
        assert_eq!(other.version(), SecretVersion::new(1));
    })
    .await;
}

/// The material comes back byte for byte: none at all, one byte, every byte value, a text with
/// multibyte characters and NULs, and the most there may be.
pub async fn material_reads_back_byte_for_byte<S: VaultStore>(store: S, _clock: FixedClock) {
    within(async {
        let owner = owner();
        let every_byte: Vec<u8> = (0..=255).collect();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("nul", vec![0]),
            ("every-byte", every_byte.clone()),
            ("text", "tökén ✓ 日本語\0\r\n\"\\".as_bytes().to_vec()),
            ("ff", vec![0xff; 4096]),
            ("most", noise(MAX_SECRET_BYTES, 7)),
            ("half", noise(MAX_SECRET_BYTES / 2 + 1, 11)),
            ("twice", [every_byte.clone(), every_byte].concat()),
        ];
        let mut written = Vec::new();
        for (hint, bytes) in &cases {
            let name = name(hint);
            let reference = store
                .put(&owner, &name, &material(bytes))
                .await
                .expect("put");
            written.push((reference, bytes.clone()));
        }
        for (reference, bytes) in written {
            let got = store.get(&owner, &reference).await.expect("it is kept");
            assert_eq!(got.expose(), bytes, "{reference}: {} bytes", bytes.len());
        }
    })
    .await;
}

/// A reference the store does not hold is refused — never `None`, never a neighbouring version, no
/// matter whether the **name** is unknown or the version is.
pub async fn a_reference_the_store_does_not_hold_is_unknown<S: VaultStore>(
    store: S,
    _clock: FixedClock,
) {
    within(async {
        let owner = owner();
        let never = name("never");
        let error = store
            .get(&owner, &VaultRef::new(never.clone(), SecretVersion::new(1)))
            .await
            .expect_err("no such name");
        assert!(matches!(error, VaultError::UnknownName), "{error:?}");
        assert_eq!(error.class(), ErrorClass::NotFound);

        let written = name("written");
        let first = store
            .put(&owner, &written, &material(b"only"))
            .await
            .unwrap();
        for version in [0, 2, 9, u64::MAX] {
            let reference = VaultRef::new(written.clone(), SecretVersion::new(version));
            let error = store
                .get(&owner, &reference)
                .await
                .expect_err("no such version");
            assert!(
                matches!(error, VaultError::UnknownVersion),
                "{reference}: {error:?}"
            );
            assert_eq!(error.class(), ErrorClass::NotFound);
        }
        // the one that exists still reads
        assert_eq!(store.get(&owner, &first).await.unwrap().expose(), b"only");
    })
    .await;
}

/// The whole of decision 2: a rotation keeps the version it superseded readable for 24 hours, and
/// refuses it from the instant the 24 hours are over — with the store's clock moved, not a sleep.
pub async fn a_superseded_version_reads_for_the_grace_and_is_refused_after<S: VaultStore>(
    store: S,
    clock: FixedClock,
) {
    within(async {
        let (owner, name) = (owner(), name("rotated"));
        let old = store.put(&owner, &name, &material(b"old")).await.unwrap();
        let now = store.put(&owner, &name, &material(b"new")).await.unwrap();
        assert_eq!(store.get(&owner, &old).await.unwrap().expose(), b"old");

        // one second before the grace is over: the old one still resolves a run
        clock.advance(grace() - Duration::from_secs(1));
        assert_eq!(
            store
                .get(&owner, &old)
                .await
                .expect("within the grace")
                .expose(),
            b"old"
        );

        // the instant it is over: refused, and the value is not returned stale
        clock.advance(Duration::from_secs(1));
        let error = store.get(&owner, &old).await.expect_err("past the grace");
        assert!(matches!(error, VaultError::UnknownVersion), "{error:?}");

        // a day later, still refused; the current version is untouched by any of it
        clock.advance(Duration::from_hours(24));
        assert!(matches!(
            store.get(&owner, &old).await.expect_err("past the grace"),
            VaultError::UnknownVersion
        ));
        assert_eq!(store.get(&owner, &now).await.unwrap().expose(), b"new");
    })
    .await;
}

/// A version that is still the current one never expires, however long it is kept: only a write
/// that supersedes it starts the clock.
pub async fn the_current_version_never_expires<S: VaultStore>(store: S, clock: FixedClock) {
    within(async {
        let (owner, name) = (owner(), name("kept"));
        let only = store.put(&owner, &name, &material(b"kept")).await.unwrap();
        clock.advance(Duration::from_hours(24 * 400));
        assert_eq!(
            store
                .get(&owner, &only)
                .await
                .expect("still current")
                .expose(),
            b"kept"
        );
    })
    .await;
}

/// `purge` removes the superseded versions the grace has passed and nothing else: not the current
/// one, not a version superseded a moment ago. A version is not given twice because a purge
/// removed it.
pub async fn purge_removes_only_the_versions_past_the_grace<S: VaultStore>(
    store: S,
    clock: FixedClock,
) {
    within(async {
        let (owner, name) = (owner(), name("purged"));
        let first = store.put(&owner, &name, &material(b"one")).await.unwrap();
        let second = store.put(&owner, &name, &material(b"two")).await.unwrap();
        // nothing is past the grace yet
        assert_eq!(store.purge().await.expect("purge"), 0);
        assert!(store.get(&owner, &first).await.is_ok());

        // the first is past it, the second is current
        clock.advance(grace() + Duration::from_secs(1));
        assert_eq!(store.purge().await.expect("purge"), 1);
        assert!(matches!(
            store.get(&owner, &first).await.expect_err("purged"),
            VaultError::UnknownVersion
        ));
        assert_eq!(store.get(&owner, &second).await.unwrap().expose(), b"two");
        // idempotent
        assert_eq!(store.purge().await.expect("the second purge"), 0);

        // the next write is the greatest the name has had, plus one: a version a purge removed is
        // never given again
        let third = store.put(&owner, &name, &material(b"three")).await.unwrap();
        assert_eq!(third.version(), SecretVersion::new(3));
        // and the version just superseded stays, because its own grace has just begun
        assert_eq!(store.purge().await.expect("purge"), 0);
        assert_eq!(store.get(&owner, &second).await.unwrap().expose(), b"two");
        assert_eq!(store.get(&owner, &third).await.unwrap().expose(), b"three");
    })
    .await;
}

/// Writers of one name at once all succeed, take versions of their own, and each reference reads
/// its own writer's material: the store serializes the writers of a name.
pub async fn concurrent_puts_take_two_versions_and_lose_nothing<S: VaultStore>(
    store: S,
    _clock: FixedClock,
) {
    within(async {
        let store = std::sync::Arc::new(store);
        let (owner, name) = (owner(), name("raced"));
        let tasks: Vec<_> = (0..6_u8)
            .map(|n| {
                let (store, owner, name) = (store.clone(), owner.clone(), name.clone());
                tokio::spawn(async move {
                    let mine = noise(512, u64::from(n) + 100);
                    let reference = store
                        .put(&owner, &name, &material(&mine))
                        .await
                        .expect("put");
                    (reference, mine)
                })
            })
            .collect();
        let mut written = Vec::new();
        for task in tasks {
            written.push(task.await.expect("the task ran"));
        }
        let mut versions: Vec<u64> = written
            .iter()
            .map(|(reference, _)| reference.version().get())
            .collect();
        versions.sort_unstable();
        versions.dedup();
        assert_eq!(versions.len(), written.len(), "no version was given twice");
        assert_eq!(versions, (1..=6).collect::<Vec<u64>>(), "1 to 6, no gap");
        for (reference, mine) in written {
            let got = store.get(&owner, &reference).await.expect("it is kept");
            assert_eq!(got.expose(), mine, "{reference}");
        }
    })
    .await;
}

/// A second owner never reads the first's secret, with the same name: their own vault, their own
/// `#v1`.
pub async fn owners_never_see_each_others_vault<S: VaultStore>(store: S, _clock: FixedClock) {
    within(async {
        let (alice, bob) = (owner(), owner());
        let name = name("shared");
        let hers = store
            .put(&alice, &name, &material(b"alice's"))
            .await
            .unwrap();
        let error = store
            .get(&bob, &hers)
            .await
            .expect_err("bob has no such secret");
        assert!(matches!(error, VaultError::UnknownName), "{error:?}");

        let his = store.put(&bob, &name, &material(b"bob's")).await.unwrap();
        assert_eq!(
            his.version(),
            SecretVersion::new(1),
            "his own first version"
        );
        assert_eq!(store.get(&bob, &his).await.unwrap().expose(), b"bob's");
        assert_eq!(store.get(&alice, &hers).await.unwrap().expose(), b"alice's");
    })
    .await;
}

/// Everything a case can make a store say, flattened to text: the error, its `Debug` and every
/// source of it.
fn everything_said_by(error: &VaultError) -> String {
    let mut said = format!("{error} {error:?}");
    let mut source = std::error::Error::source(error);
    while let Some(e) = source {
        said.push_str(&format!(" {e} {e:?}"));
        source = e.source();
    }
    said
}

/// No `Debug` of the material, of a reference or of what a call returned shows the material, and
/// the errors a caller can provoke (an unknown name, an unknown version, one past its grace) do not
/// either: the sentinel planted in the material is nowhere, neither as it is nor in hex.
pub async fn no_error_or_debug_text_shows_a_secret<S: VaultStore>(store: S, clock: FixedClock) {
    within(async {
        let owner = owner();
        let the_name = name("sentinel");
        let other = name("absent");
        let planted = material(format!("{{\"api_key\":\"{SENTINEL}\"}}").as_bytes());
        let mut said = vec![
            format!("{planted:?}"),
            format!("{owner:?} {the_name:?} {other:?}"),
        ];

        let put = store.put(&owner, &the_name, &planted).await;
        said.push(format!("{put:?}"));
        let first = put.expect("put");
        said.push(format!("{first:?} {first}"));
        said.push(format!("{:?}", store.get(&owner, &first).await));

        // the errors a caller can provoke: the first version past its grace, an unknown name, an
        // unknown version
        store
            .put(&owner, &the_name, &planted)
            .await
            .expect("put again");
        clock.advance(grace() + Duration::from_secs(1));
        for error in [
            store
                .get(&owner, &first)
                .await
                .expect_err("superseded past the grace"),
            store
                .get(&owner, &VaultRef::new(other, SecretVersion::new(1)))
                .await
                .expect_err("no such name"),
            store
                .get(
                    &owner,
                    &VaultRef::new(the_name.clone(), SecretVersion::new(99)),
                )
                .await
                .expect_err("no such version"),
        ] {
            said.push(everything_said_by(&error));
        }

        let hex: String = SENTINEL.bytes().map(|b| format!("{b:02x}")).collect();
        for text in said {
            assert!(!text.contains(SENTINEL), "the secret is in: {text}");
            assert!(!text.contains(&hex), "the secret, in hex, is in: {text}");
        }
    })
    .await;
}
