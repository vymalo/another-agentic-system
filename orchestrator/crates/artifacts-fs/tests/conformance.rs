//! The `ArtifactStore` testkit against a directory, and what only a directory has: modes, atomic
//! writes that leave nothing behind, files that survive a reopen, damage that is reported and not
//! served.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::Path;

use bytes::Bytes;
use futures::StreamExt as _;
use orch_artifacts_fs::FsArtifacts;
use orch_core::{Classify as _, ErrorClass, ThreadId};
use orch_ports::{ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream};
use tempfile::TempDir;

/// A store over a directory of its own, removed with the store.
struct Held {
    store: FsArtifacts,
    _dir: TempDir,
}

impl ArtifactStore for Held {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        self.store.put(key, bytes, meta).await
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        self.store.get(key).await
    }

    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        self.store.delete(key).await
    }
}

async fn make() -> Option<Held> {
    let dir = TempDir::new().unwrap();
    let store = FsArtifacts::open(dir.path()).await.unwrap();
    Some(Held { store, _dir: dir })
}

orch_ports::artifact_store_conformance!(make);

fn thread() -> ThreadId {
    ThreadId(uuid::Uuid::now_v7())
}

fn file(bytes: &[u8], name: Option<&str>) -> (ArtifactKey, ArtifactMeta, Bytes) {
    let meta = ArtifactMeta::of("image/png", name.map(str::to_owned), bytes);
    (meta.key(thread()), meta, Bytes::copy_from_slice(bytes))
}

/// Every file under `dir`, recursively, as paths.
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

async fn read_all(store: &FsArtifacts, key: &ArtifactKey) -> Option<(ArtifactMeta, Vec<u8>)> {
    let (meta, mut stream) = store.get(key).await.unwrap()?;
    let mut all = Vec::new();
    while let Some(piece) = stream.next().await {
        all.extend_from_slice(&piece.unwrap());
    }
    Some((meta, all))
}

#[tokio::test]
async fn a_put_leaves_the_bytes_and_their_meta_and_no_temporary_file() {
    let dir = TempDir::new().unwrap();
    let store = FsArtifacts::open(dir.path()).await.unwrap();
    let (key, meta, bytes) = file(b"png bytes", Some("chart.png"));
    store.put(&key, bytes, &meta).await.unwrap();

    let hash = key.sha256_hex();
    let thread_dir = dir.path().join("threads").join(key.thread().to_string());
    assert_eq!(
        walk(dir.path()),
        [
            thread_dir.join(&hash),
            thread_dir.join(format!("{hash}.meta.json")),
        ]
    );
    assert_eq!(std::fs::read(thread_dir.join(&hash)).unwrap(), b"png bytes");
    let sidecar: serde_json::Value = serde_json::from_slice(
        &std::fs::read(thread_dir.join(format!("{hash}.meta.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(sidecar["mediaType"], "image/png");
    assert_eq!(sidecar["filename"], "chart.png");
    assert_eq!(sidecar["size"], 9);
    assert_eq!(sidecar["sha256"], hash);
}

#[cfg(unix)]
#[tokio::test]
async fn files_are_private_to_the_owner() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = TempDir::new().unwrap();
    let root = dir.path().join("a").join("artifacts");
    let store = FsArtifacts::open(&root).await.unwrap();
    let (key, meta, bytes) = file(b"secret chart", None);
    store.put(&key, bytes, &meta).await.unwrap();

    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    for path in walk(&root) {
        assert_eq!(mode(&path), 0o600, "{path:?}");
    }
    // the directories the store made are the owner's alone, the root included
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&root.join("threads")), 0o700);
    assert_eq!(
        mode(&root.join("threads").join(key.thread().to_string())),
        0o700
    );
}

#[tokio::test]
async fn files_survive_a_reopen_of_the_same_root() {
    let dir = TempDir::new().unwrap();
    let (key, meta, bytes) = file(b"kept", Some("n.txt"));
    FsArtifacts::open(dir.path())
        .await
        .unwrap()
        .put(&key, bytes, &meta)
        .await
        .unwrap();
    let again = FsArtifacts::open(dir.path()).await.unwrap();
    let (got, bytes) = read_all(&again, &key).await.unwrap();
    assert_eq!(got, meta);
    assert_eq!(bytes, b"kept");
}

#[tokio::test]
async fn a_put_of_a_file_that_is_there_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let store = FsArtifacts::open(dir.path()).await.unwrap();
    let (key, meta, bytes) = file(b"once", Some("n.txt"));
    store.put(&key, bytes.clone(), &meta).await.unwrap();
    let before = walk(dir.path())
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().modified().unwrap())
        .collect::<Vec<_>>();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    store.put(&key, bytes, &meta).await.unwrap();
    let after = walk(dir.path())
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().modified().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(before, after);
}

#[tokio::test]
async fn a_second_name_for_the_same_content_replaces_the_meta_and_keeps_the_bytes() {
    let dir = TempDir::new().unwrap();
    let store = FsArtifacts::open(dir.path()).await.unwrap();
    let thread = thread();
    let first = ArtifactMeta::of("text/plain", Some("one.txt".into()), b"same");
    let second = ArtifactMeta::of("text/plain", Some("two.txt".into()), b"same");
    let key = first.key(thread);
    store
        .put(&key, Bytes::from_static(b"same"), &first)
        .await
        .unwrap();
    store
        .put(&key, Bytes::from_static(b"same"), &second)
        .await
        .unwrap();
    let (got, bytes) = read_all(&store, &key).await.unwrap();
    assert_eq!(got.filename.as_deref(), Some("two.txt"));
    assert_eq!(bytes, b"same");
    assert_eq!(walk(dir.path()).len(), 2);
}

#[tokio::test]
async fn bytes_without_their_meta_and_a_meta_without_bytes_are_not_found() {
    let dir = TempDir::new().unwrap();
    let store = FsArtifacts::open(dir.path()).await.unwrap();
    let (key, meta, bytes) = file(b"half", None);
    store.put(&key, bytes.clone(), &meta).await.unwrap();
    let thread_dir = dir.path().join("threads").join(key.thread().to_string());
    let hash = key.sha256_hex();

    std::fs::remove_file(thread_dir.join(format!("{hash}.meta.json"))).unwrap();
    assert!(store.get(&key).await.unwrap().is_none());
    store.put(&key, bytes, &meta).await.unwrap();
    assert!(store.get(&key).await.unwrap().is_some(), "a put repairs it");

    std::fs::remove_file(thread_dir.join(&hash)).unwrap();
    assert!(store.get(&key).await.unwrap().is_none());
    // and the put that finds a meta without bytes writes the bytes
    store
        .put(&key, Bytes::from_static(b"half"), &meta)
        .await
        .unwrap();
    assert!(store.get(&key).await.unwrap().is_some());
}

#[tokio::test]
async fn damage_is_reported_and_never_served() {
    let dir = TempDir::new().unwrap();
    let store = FsArtifacts::open(dir.path()).await.unwrap();
    let (key, meta, bytes) = file(b"twelve bytes", None);
    store.put(&key, bytes, &meta).await.unwrap();
    let thread_dir = dir.path().join("threads").join(key.thread().to_string());
    let hash = key.sha256_hex();

    // the bytes were cut short
    std::fs::write(thread_dir.join(&hash), b"twelve").unwrap();
    let err = store.get(&key).await.err().expect("an error");
    assert!(matches!(err, ArtifactError::Corrupt(_)), "{err}");
    assert_eq!(err.class(), ErrorClass::Corrupt);
    assert!(err.class().should_alert());

    // the meta is not JSON, or names another hash
    std::fs::write(thread_dir.join(&hash), b"twelve bytes").unwrap();
    for damaged in [
        "not json".to_owned(),
        r#"{"mediaType":"text/plain","size":12,"sha256":"00","filename":null}"#.to_owned(),
        r#"{"mediaType":"text/plain","size":12,"sha256":"x","filename":null,"extra":1}"#.to_owned(),
    ] {
        std::fs::write(thread_dir.join(format!("{hash}.meta.json")), damaged).unwrap();
        let err = store.get(&key).await.err().expect("an error");
        assert!(matches!(err, ArtifactError::Corrupt(_)), "{err}");
    }
}

#[tokio::test]
async fn a_root_that_cannot_be_used_is_found_when_it_is_opened() {
    let dir = TempDir::new().unwrap();
    // a file where the directory should be
    let blocked = dir.path().join("a-file");
    std::fs::write(&blocked, b"x").unwrap();
    let err = FsArtifacts::open(&blocked).await.unwrap_err();
    assert!(err.to_string().contains("a-file"), "{err}");
    let err = FsArtifacts::open(blocked.join("below")).await.unwrap_err();
    assert!(err.to_string().contains("cannot use"), "{err}");
    // a new directory, parents and all, is made
    let made = dir.path().join("x").join("y").join("z");
    assert_eq!(FsArtifacts::open(&made).await.unwrap().root(), made);
    assert!(made.is_dir());
}

#[tokio::test]
async fn a_write_that_fails_is_transient_and_leaves_no_temporary_file() {
    let dir = TempDir::new().unwrap();
    let store = FsArtifacts::open(dir.path()).await.unwrap();
    let (key, meta, bytes) = file(b"cannot land", None);
    // a plain file where the thread's directory belongs: nothing can be written under it
    std::fs::create_dir_all(dir.path().join("threads")).unwrap();
    std::fs::write(
        dir.path().join("threads").join(key.thread().to_string()),
        b"in the way",
    )
    .unwrap();
    let err = store.put(&key, bytes, &meta).await.unwrap_err();
    assert!(matches!(err, ArtifactError::Unavailable { .. }), "{err}");
    assert!(err.is_retryable());
    assert!(
        !err.to_string().contains(dir.path().to_str().unwrap()),
        "an error never shows a path: {err}"
    );
    assert_eq!(walk(dir.path()).len(), 1);
}
