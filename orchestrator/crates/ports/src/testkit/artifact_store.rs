//! `ArtifactStore` conformance cases: what the worker and the API rely on from any store, stated
//! once and run against the in-memory store, the directory store and the S3 store.
//!
//! An implementation supplies a fresh, isolated store (`async fn() -> Option<S>`; `None` skips the
//! suite, as for a bucket nobody gave the test). Each case is `async fn(store)` and gives up after
//! 60 seconds. A case uses thread ids of its own, so a store shared by the tests of one run (a
//! bucket) is never confused by the case beside it.
//!
//! What is deliberately not asserted: where or how the bytes are kept, the size of the pieces a
//! file is streamed in, and the wording of any error.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt as _;
use orch_core::{Classify, ErrorClass, ThreadId};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// The size of the file of the streaming case: well over any buffer a store or a server has, so a
/// store that reads it whole is at least noticed in a profile.
pub const LARGE_FILE_BYTES: usize = 8 * 1024 * 1024;

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A thread no other case uses.
fn thread() -> ThreadId {
    ThreadId(Uuid::now_v7())
}

/// `len` bytes that are not repeating (a xorshift sequence), so a store that drops, repeats or
/// reorders a piece does not give the same hash back.
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

/// A file: its bytes, its meta and its key in `thread`.
fn file(
    thread: ThreadId,
    bytes: Vec<u8>,
    media_type: &str,
    filename: Option<&str>,
) -> (ArtifactKey, ArtifactMeta, Bytes) {
    let meta = ArtifactMeta::of(media_type, filename.map(str::to_owned), &bytes);
    (meta.key(thread), meta, Bytes::from(bytes))
}

/// What a stream gave, without keeping it: its size, its hash and how many pieces it came in.
struct Drained {
    size: u64,
    sha256: [u8; 32],
    pieces: usize,
}

/// Reads the stream to its end, hashing each piece as it comes. The case never holds the file.
async fn drain(mut stream: ByteStream) -> Drained {
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut pieces = 0usize;
    while let Some(piece) = stream.next().await {
        let piece = piece.expect("a piece of the file");
        size += piece.len() as u64;
        hash.update(&piece);
        pieces += 1;
    }
    Drained {
        size,
        sha256: hash.finalize().into(),
        pieces,
    }
}

/// The whole of a small file, for a case that compares bytes.
async fn read_all(mut stream: ByteStream) -> Vec<u8> {
    let mut all = Vec::new();
    while let Some(piece) = stream.next().await {
        all.extend_from_slice(&piece.expect("a piece of the file"));
    }
    all
}

async fn found<S: ArtifactStore>(store: &S, key: &ArtifactKey) -> (ArtifactMeta, ByteStream) {
    store
        .get(key)
        .await
        .expect("get does not fail")
        .expect("the file is kept")
}

/// What was put is what comes back: the same meta and the same bytes.
pub async fn put_then_get_round_trips<S: ArtifactStore>(store: S) {
    within(async {
        let (key, meta, bytes) = file(thread(), noise(100_000, 1), "image/png", Some("chart.png"));
        store.put(&key, bytes.clone(), &meta).await.expect("put");
        let (got, stream) = found(&store, &key).await;
        assert_eq!(got, meta);
        assert_eq!(read_all(stream).await, bytes.to_vec());
    })
    .await;
}

/// A file of no bytes is a file.
pub async fn an_empty_file_round_trips<S: ArtifactStore>(store: S) {
    within(async {
        let (key, meta, bytes) = file(thread(), Vec::new(), "text/plain", None);
        store.put(&key, bytes, &meta).await.expect("put");
        let (got, stream) = found(&store, &key).await;
        assert_eq!(got.size, 0);
        assert_eq!(got, meta);
        assert!(read_all(stream).await.is_empty());
    })
    .await;
}

/// The meta comes back whole: the media type as given, the size, the hash, and a file name with
/// spaces, quotes and other scripts; a file with no name has none.
pub async fn the_meta_is_kept_whole<S: ArtifactStore>(store: S) {
    within(async {
        let thread = thread();
        let names = [
            None,
            Some("report.pdf"),
            Some("My résumé (final) \"v2\" 日本語 ✓.txt"),
            Some("100% a+b=c; d&e?f#g.csv"),
        ];
        for (i, name) in names.into_iter().enumerate() {
            let (key, meta, bytes) = file(
                thread,
                noise(1000 + i, 10 + i as u64),
                "application/vnd.example+json; charset=utf-8",
                name,
            );
            store.put(&key, bytes, &meta).await.expect("put");
            let (got, _) = found(&store, &key).await;
            assert_eq!(got.media_type, meta.media_type, "{name:?}");
            assert_eq!(got.filename, meta.filename, "{name:?}");
            assert_eq!(got.size, meta.size, "{name:?}");
            assert_eq!(got.sha256, meta.sha256, "{name:?}");
        }
    })
    .await;
}

/// Putting a file again is not an error and changes nothing: not the bytes, not the meta.
pub async fn putting_twice_is_the_same_as_once<S: ArtifactStore>(store: S) {
    within(async {
        let (key, meta, bytes) = file(thread(), noise(5000, 2), "text/plain", Some("a.txt"));
        for _ in 0..3 {
            store.put(&key, bytes.clone(), &meta).await.expect("put");
        }
        let (got, stream) = found(&store, &key).await;
        assert_eq!(got, meta);
        assert_eq!(read_all(stream).await, bytes.to_vec());
    })
    .await;
}

/// The same content put again under another name is kept once, under one of the two names, whole.
pub async fn the_same_content_under_another_name_is_one_file<S: ArtifactStore>(store: S) {
    within(async {
        let thread = thread();
        let content = noise(3000, 3);
        let (key, first, bytes) = file(thread, content.clone(), "text/plain", Some("one.txt"));
        let (_, second, _) = file(thread, content, "text/plain", Some("two.txt"));
        store.put(&key, bytes.clone(), &first).await.expect("put");
        store.put(&key, bytes.clone(), &second).await.expect("put");
        let (got, stream) = found(&store, &key).await;
        assert!(got == first || got == second, "{got:?}");
        assert_eq!(read_all(stream).await, bytes.to_vec());
    })
    .await;
}

/// A key nothing was put under has no file, and asking is not an error; neither is removing it.
pub async fn a_missing_key_is_none<S: ArtifactStore>(store: S) {
    within(async {
        let (key, _, _) = file(thread(), noise(10, 4), "text/plain", None);
        assert!(store.get(&key).await.expect("get").is_none());
        store.delete(&key).await.expect("delete of a missing file");
        assert!(store.get(&key).await.expect("get").is_none());
    })
    .await;
}

/// A file of several MiB is read as a stream, to the byte, and the case never holds it whole: it
/// hashes the pieces as they come.
pub async fn a_large_file_streams<S: ArtifactStore>(store: S) {
    within(async {
        let (key, meta, bytes) = file(
            thread(),
            noise(LARGE_FILE_BYTES, 5),
            "application/octet-stream",
            Some("big.bin"),
        );
        store.put(&key, bytes, &meta).await.expect("put");
        let (got, stream) = found(&store, &key).await;
        assert_eq!(got.size, LARGE_FILE_BYTES as u64);
        let drained = drain(stream).await;
        assert_eq!(drained.size, meta.size);
        assert_eq!(drained.sha256, meta.sha256, "the stream is not the file");
        assert!(drained.pieces >= 1);
    })
    .await;
}

/// Puts of one key at once all succeed and leave one whole file.
pub async fn concurrent_puts_of_one_key_leave_one_whole_file<S: ArtifactStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        let (key, meta, bytes) = file(thread(), noise(1_500_000, 6), "image/png", Some("same.png"));
        let puts: Vec<_> = (0..16)
            .map(|_| {
                let store = Arc::clone(&store);
                let (bytes, meta) = (bytes.clone(), meta.clone());
                tokio::spawn(async move { store.put(&key, bytes, &meta).await })
            })
            .collect();
        for put in puts {
            put.await.expect("the task").expect("every put succeeds");
        }
        let (got, stream) = found(&*store, &key).await;
        assert_eq!(got, meta);
        let drained = drain(stream).await;
        assert_eq!((drained.size, drained.sha256), (meta.size, meta.sha256));
    })
    .await;
}

/// Puts of different files at once keep each one.
pub async fn concurrent_puts_of_different_files_keep_each<S: ArtifactStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        let thread = thread();
        let files: Vec<_> = (0..8u64)
            .map(|i| {
                file(
                    thread,
                    noise(200_000 + i as usize, 100 + i),
                    "text/plain",
                    None,
                )
            })
            .collect();
        let puts: Vec<_> = files
            .iter()
            .map(|(key, meta, bytes)| {
                let store = Arc::clone(&store);
                let (key, meta, bytes) = (*key, meta.clone(), bytes.clone());
                tokio::spawn(async move { store.put(&key, bytes, &meta).await })
            })
            .collect();
        for put in puts {
            put.await.expect("the task").expect("put");
        }
        for (key, meta, bytes) in &files {
            let (got, stream) = found(&*store, key).await;
            assert_eq!(&got, meta);
            assert_eq!(read_all(stream).await, bytes.to_vec());
        }
    })
    .await;
}

/// A deleted file is gone, deleting it again is fine, and the file beside it is untouched.
pub async fn delete_removes_the_file_and_only_that_file<S: ArtifactStore>(store: S) {
    within(async {
        let thread = thread();
        let (gone, gone_meta, gone_bytes) = file(thread, noise(2000, 7), "text/plain", None);
        let (kept, kept_meta, kept_bytes) = file(thread, noise(2000, 8), "text/plain", None);
        store.put(&gone, gone_bytes, &gone_meta).await.expect("put");
        store
            .put(&kept, kept_bytes.clone(), &kept_meta)
            .await
            .expect("put");
        store.delete(&gone).await.expect("delete");
        assert!(store.get(&gone).await.expect("get").is_none());
        store.delete(&gone).await.expect("delete again");
        let (got, stream) = found(&store, &kept).await;
        assert_eq!(got, kept_meta);
        assert_eq!(read_all(stream).await, kept_bytes.to_vec());
        // and it can be put again
        let (key, meta, bytes) = file(thread, noise(2000, 7), "text/plain", None);
        store
            .put(&key, bytes, &meta)
            .await
            .expect("put after delete");
        assert!(store.get(&key).await.expect("get").is_some());
    })
    .await;
}

/// The same content in two threads is two files: the key holds the thread, and removing one
/// leaves the other.
pub async fn threads_do_not_share_a_file<S: ArtifactStore>(store: S) {
    within(async {
        let content = noise(4000, 9);
        let (one, meta, bytes) = file(thread(), content.clone(), "text/plain", Some("n.txt"));
        let (two, _, _) = file(thread(), content, "text/plain", Some("n.txt"));
        assert_ne!(one, two);
        store.put(&one, bytes.clone(), &meta).await.expect("put");
        assert!(
            store.get(&two).await.expect("get").is_none(),
            "a thread read a file of another thread"
        );
        store.put(&two, bytes.clone(), &meta).await.expect("put");
        store.delete(&one).await.expect("delete");
        assert!(store.get(&one).await.expect("get").is_none());
        let (got, stream) = found(&store, &two).await;
        assert_eq!(got, meta);
        assert_eq!(read_all(stream).await, bytes.to_vec());
    })
    .await;
}

/// A store keeps only what its key says it is: bytes that do not hash to the key, a meta that does
/// not describe them, or a name a header cannot carry are refused as invalid, and nothing is kept.
pub async fn bytes_that_are_not_the_key_are_refused<S: ArtifactStore>(store: S) {
    within(async {
        let thread = thread();
        let (key, meta, bytes) = file(thread, noise(3000, 11), "text/plain", Some("f.txt"));

        let mut other = bytes.to_vec();
        other[0] ^= 0xff;
        let wrong_bytes = store
            .put(&key, Bytes::from(other), &meta)
            .await
            .expect_err("bytes of another hash");
        let mut short = meta.clone();
        short.size -= 1;
        let wrong_size = store
            .put(&key, bytes.clone(), &short)
            .await
            .expect_err("a meta with another size");
        let mut lie = meta.clone();
        lie.sha256[0] ^= 1;
        let wrong_hash = store
            .put(&key, bytes.clone(), &lie)
            .await
            .expect_err("a meta with another hash");
        let mut header = meta.clone();
        header.media_type = "text/plain\r\nX-Injected: 1".to_owned();
        let wrong_type = store
            .put(&key, bytes.clone(), &header)
            .await
            .expect_err("a media type that is not plain text");
        let mut name = meta.clone();
        name.filename = Some("a\nb".to_owned());
        let wrong_name = store
            .put(&key, bytes.clone(), &name)
            .await
            .expect_err("a file name with a control character");
        for err in [wrong_bytes, wrong_size, wrong_hash, wrong_type, wrong_name] {
            assert!(matches!(err, ArtifactError::Invalid(_)), "{err}");
            assert_eq!(err.class(), ErrorClass::Invalid);
            assert!(!err.is_retryable());
        }
        assert!(
            store.get(&key).await.expect("get").is_none(),
            "a refused put kept something"
        );
    })
    .await;
}

/// A copy is a file of its own under the new key: same meta, same bytes, and it outlives the
/// source. The source is untouched, and the file beside it in the new thread is not disturbed.
pub async fn a_copy_is_a_file_of_its_own<S: ArtifactStore>(store: S) {
    within(async {
        let (from, meta, bytes) = file(thread(), noise(70_000, 12), "image/png", Some("c.png"));
        let to = meta.key(thread());
        let (beside, beside_meta, beside_bytes) =
            file(to.thread(), noise(500, 13), "text/plain", None);
        store.put(&from, bytes.clone(), &meta).await.expect("put");
        store
            .put(&beside, beside_bytes.clone(), &beside_meta)
            .await
            .expect("put");
        assert!(store.get(&to).await.expect("get").is_none());

        store.copy(&from, &to).await.expect("copy");
        let (got, stream) = found(&store, &to).await;
        assert_eq!(got, meta);
        assert_eq!(read_all(stream).await, bytes.to_vec());
        let (got, stream) = found(&store, &from).await;
        assert_eq!(got, meta);
        assert_eq!(read_all(stream).await, bytes.to_vec());

        // the copy outlives the source
        store.delete(&from).await.expect("delete the source");
        assert!(store.get(&from).await.expect("get").is_none());
        let (got, stream) = found(&store, &to).await;
        assert_eq!(got, meta);
        assert_eq!(read_all(stream).await, bytes.to_vec());
        let (got, stream) = found(&store, &beside).await;
        assert_eq!(got, beside_meta);
        assert_eq!(read_all(stream).await, beside_bytes.to_vec());

        // and deleting the copy does not reach back to a source put again
        store
            .put(&from, bytes.clone(), &meta)
            .await
            .expect("put again");
        store.delete(&to).await.expect("delete the copy");
        assert!(store.get(&to).await.expect("get").is_none());
        let (_, stream) = found(&store, &from).await;
        assert_eq!(read_all(stream).await, bytes.to_vec());
    })
    .await;
}

/// Copying again, and copying a file onto its own key, succeed and change nothing.
pub async fn copying_twice_is_the_same_as_once<S: ArtifactStore>(store: S) {
    within(async {
        let (from, meta, bytes) = file(thread(), noise(4000, 14), "text/plain", Some("a.txt"));
        let to = meta.key(thread());
        store.put(&from, bytes.clone(), &meta).await.expect("put");
        for _ in 0..3 {
            store.copy(&from, &to).await.expect("copy");
        }
        store.copy(&from, &from).await.expect("copy onto itself");
        for key in [&from, &to] {
            let (got, stream) = found(&store, key).await;
            assert_eq!(got, meta);
            assert_eq!(read_all(stream).await, bytes.to_vec());
        }
    })
    .await;
}

/// Copies of one file to several threads at once all succeed and leave each one whole.
pub async fn concurrent_copies_of_one_file_leave_each_whole<S: ArtifactStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        let (from, meta, bytes) = file(thread(), noise(300_000, 15), "image/png", Some("n.png"));
        store.put(&from, bytes.clone(), &meta).await.expect("put");
        let targets: Vec<ArtifactKey> = (0..8).map(|_| meta.key(thread())).collect();
        let copies: Vec<_> = targets
            .iter()
            // two copies of each target race: the second is the idempotent one
            .chain(targets.iter())
            .map(|to| {
                let store = Arc::clone(&store);
                let to = *to;
                tokio::spawn(async move { store.copy(&from, &to).await })
            })
            .collect();
        for copy in copies {
            copy.await.expect("the task").expect("every copy succeeds");
        }
        for to in &targets {
            let (got, stream) = found(&*store, to).await;
            assert_eq!(got, meta);
            assert_eq!(read_all(stream).await, bytes.to_vec());
        }
    })
    .await;
}

/// A source that is not there is [`ArtifactError::NotFound`], and nothing appears under the new key.
pub async fn copying_a_missing_file_is_not_found<S: ArtifactStore>(store: S) {
    within(async {
        let (from, meta, _) = file(thread(), noise(100, 16), "text/plain", None);
        let to = meta.key(thread());
        let err = store.copy(&from, &to).await.expect_err("no such file");
        assert!(matches!(err, ArtifactError::NotFound), "{err}");
        assert_eq!(err.class(), ErrorClass::NotFound);
        assert!(store.get(&to).await.expect("get").is_none());

        // a file that was there and was deleted is missing too
        let (gone, gone_meta, gone_bytes) = file(thread(), noise(100, 23), "text/plain", None);
        store.put(&gone, gone_bytes, &gone_meta).await.expect("put");
        store.delete(&gone).await.expect("delete");
        let err = store
            .copy(&gone, &gone_meta.key(thread()))
            .await
            .expect_err("deleted");
        assert!(matches!(err, ArtifactError::NotFound), "{err}");
    })
    .await;
}

/// A copy keeps the content: two keys that hold different hashes are refused as invalid, and
/// nothing is kept under the second.
pub async fn a_copy_to_another_hash_is_refused<S: ArtifactStore>(store: S) {
    within(async {
        let (from, meta, bytes) = file(thread(), noise(100, 18), "text/plain", None);
        let (other, _, _) = file(thread(), noise(100, 21), "text/plain", None);
        store.put(&from, bytes, &meta).await.expect("put");
        let err = store.copy(&from, &other).await.expect_err("another hash");
        assert!(matches!(err, ArtifactError::Invalid(_)), "{err}");
        assert_eq!(err.class(), ErrorClass::Invalid);
        assert!(store.get(&other).await.expect("get").is_none());
    })
    .await;
}
