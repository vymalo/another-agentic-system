//! The migration of a directory store into another store (ADR 0058 A5.4): what it copies, what it
//! refuses to copy and says so, that a second run changes nothing, that a dry run writes nothing, and
//! that it never writes to the source or deletes from either side.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use orch_artifacts_fs::{
    FailureKind, FsArtifacts, IgnoredWhy, MAX_CONSECUTIVE_TARGET_FAILURES, Options, Report, migrate,
};
use orch_core::ThreadId;
use orch_ports::memory::MemoryArtifacts;
use orch_ports::{
    ArtifactError, ArtifactHead, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, PresignedPut,
};
use tempfile::TempDir;
use uuid::Uuid;

const T1: u128 = 0x0190_0000_0000_7000_8000_0000_0000_0001;
const T2: u128 = 0x0190_0000_0000_7000_8000_0000_0000_0002;

fn thread(n: u128) -> ThreadId {
    ThreadId(Uuid::from_u128(n))
}

/// A file: its key in `thread`, its meta and its bytes.
fn file(n: u128, content: &[u8], name: Option<&str>) -> (ArtifactKey, ArtifactMeta, Bytes) {
    let meta = ArtifactMeta::of("text/plain", name.map(str::to_owned), content);
    (meta.key(thread(n)), meta, Bytes::copy_from_slice(content))
}

/// A directory store that holds `files`, in a directory of its own.
struct Source {
    dir: TempDir,
    store: FsArtifacts,
}

impl Source {
    async fn with(files: &[(ArtifactKey, ArtifactMeta, Bytes)]) -> Self {
        let dir = TempDir::new().unwrap();
        let store = FsArtifacts::open(dir.path()).await.unwrap();
        for (key, meta, bytes) in files {
            store.put(key, bytes.clone(), meta).await.unwrap();
        }
        Source { dir, store }
    }

    fn blob(&self, key: &ArtifactKey) -> std::path::PathBuf {
        self.dir
            .path()
            .join("threads")
            .join(key.thread().to_string())
            .join(key.sha256_hex())
    }

    fn meta(&self, key: &ArtifactKey) -> std::path::PathBuf {
        let mut path = self.blob(key).into_os_string();
        path.push(".meta.json");
        path.into()
    }

    /// Every path under the root with its size, to see that nothing was written or removed.
    fn listing(&self) -> BTreeMap<String, u64> {
        fn walk(dir: &Path, root: &Path, into: &mut BTreeMap<String, u64>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let meta = std::fs::symlink_metadata(entry.path()).unwrap();
                let name = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                into.insert(name, meta.len());
                if meta.is_dir() {
                    walk(&entry.path(), root, into);
                }
            }
        }
        let mut into = BTreeMap::new();
        walk(self.dir.path(), self.dir.path(), &mut into);
        into
    }
}

fn options() -> Options {
    Options {
        dry_run: false,
        max_file_bytes: 10 * 1024 * 1024,
    }
}

async fn run<D: ArtifactStore>(source: &Source, to: &D, options: Options) -> Report {
    migrate(&source.store, to, options, |_, _| {})
        .await
        .unwrap()
}

/// A bucket that says the checksum of what it holds, like the S3 of a `PUT` that signed one.
fn bucket() -> MemoryArtifacts {
    MemoryArtifacts::presigning()
}

#[tokio::test]
async fn every_file_is_put_under_its_own_key_with_its_meta_and_the_report_counts_them() {
    let files = [
        file(T1, b"hello", Some("hello.txt")),
        file(T1, b"", None),
        file(T2, b"another thread", Some("café.txt")),
    ];
    let source = Source::with(&files).await;
    let target = bucket();
    let mut seen = Vec::new();

    let report = migrate(&source.store, &target, options(), |key, outcome| {
        seen.push((*key, outcome.clone()));
    })
    .await
    .unwrap();

    assert!(report.is_complete(), "{report:?}");
    assert_eq!((report.files, report.uploaded, report.skipped), (3, 3, 0));
    assert_eq!(report.bytes_uploaded, 5 + 14);
    assert!(report.failed.is_empty() && report.ignored.is_empty());
    assert_eq!(seen.len(), 3, "the observer is told of every file");
    for (key, meta, bytes) in &files {
        let (found, stream) = target.get(key).await.unwrap().expect("in the bucket");
        assert_eq!(&found, meta, "the meta is the one the directory kept");
        let got: Vec<u8> = futures::TryStreamExt::try_collect::<Vec<Bytes>>(stream)
            .await
            .unwrap()
            .concat();
        assert_eq!(got, bytes.to_vec());
    }
}

#[tokio::test]
async fn a_second_run_skips_every_file_and_changes_nothing() {
    let files = [file(T1, b"one", Some("one.txt")), file(T2, b"two", None)];
    let source = Source::with(&files).await;
    let target = bucket();
    run(&source, &target, options()).await;
    let before = source.listing();

    let again = run(&source, &target, options()).await;

    assert!(again.is_complete());
    assert_eq!((again.files, again.uploaded, again.skipped), (2, 0, 2));
    assert_eq!(again.bytes_skipped, 6);
    assert_eq!(
        source.listing(),
        before,
        "the source is read, never written"
    );
    assert_eq!(target.len(), 2);
}

#[tokio::test]
async fn a_dry_run_reads_and_checks_and_writes_nothing() {
    let files = [file(T1, b"one", None), file(T2, b"two", None)];
    let source = Source::with(&files).await;
    let target = bucket();
    let before = source.listing();

    let report = run(
        &source,
        &target,
        Options {
            dry_run: true,
            ..options()
        },
    )
    .await;

    assert!(report.dry_run && report.is_complete());
    assert_eq!(
        (report.files, report.would_upload, report.uploaded),
        (2, 2, 0)
    );
    assert_eq!(report.bytes_would_upload, 6);
    assert!(target.is_empty(), "a dry run writes nothing to the bucket");
    assert_eq!(source.listing(), before);
}

#[tokio::test]
async fn a_dry_run_after_the_real_one_would_upload_nothing() {
    let files = [file(T1, b"one", None)];
    let source = Source::with(&files).await;
    let target = bucket();
    run(&source, &target, options()).await;

    let report = run(
        &source,
        &target,
        Options {
            dry_run: true,
            ..options()
        },
    )
    .await;

    assert_eq!((report.would_upload, report.skipped), (0, 1));
}

#[tokio::test]
async fn bytes_that_do_not_hash_to_their_key_are_failed_and_never_uploaded() {
    let good = file(T1, b"good", None);
    let bad = file(T1, b"the original", None);
    let source = Source::with(&[good.clone(), bad.clone()]).await;
    // same length, other bytes: only the hash can tell
    std::fs::write(source.blob(&bad.0), b"the tampered").unwrap();
    let target = bucket();

    let report = run(&source, &target, options()).await;

    assert!(!report.is_complete());
    assert_eq!(
        (report.files, report.uploaded, report.failed.len()),
        (2, 1, 1)
    );
    let failure = &report.failed[0];
    assert_eq!(failure.key, bad.0);
    assert_eq!(failure.kind, FailureKind::Corrupt);
    assert!(failure.reason.contains("hash"), "{}", failure.reason);
    assert!(
        !target.contains(&bad.0),
        "damaged bytes never reach the bucket"
    );
    assert!(
        target.contains(&good.0),
        "the others are migrated all the same"
    );
}

#[tokio::test]
async fn a_file_that_is_not_the_size_its_meta_says_is_corrupt_not_uploaded() {
    let short = file(T1, b"complete content", None);
    let source = Source::with(std::slice::from_ref(&short)).await;
    std::fs::write(source.blob(&short.0), b"complete").unwrap();
    let target = bucket();

    let report = run(&source, &target, options()).await;

    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].kind, FailureKind::Corrupt);
    assert!(target.is_empty());
}

#[tokio::test]
async fn a_meta_that_fails_the_check_is_failed_and_the_bytes_are_not_uploaded() {
    let named = file(T1, b"content", Some("fine.txt"));
    let source = Source::with(std::slice::from_ref(&named)).await;
    // a file name with a control character is not text a header can carry (`ArtifactMeta::check`)
    let sidecar = format!(
        r#"{{"mediaType":"text/plain","size":7,"sha256":"{}","filename":"bad\u0007name"}}"#,
        named.0.sha256_hex()
    );
    std::fs::write(source.meta(&named.0), sidecar).unwrap();
    let target = bucket();

    let report = run(&source, &target, options()).await;

    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].kind, FailureKind::Invalid);
    assert!(target.is_empty());
}

#[tokio::test]
async fn a_meta_that_cannot_be_read_is_a_corrupt_file() {
    let one = file(T1, b"content", None);
    let source = Source::with(std::slice::from_ref(&one)).await;
    std::fs::write(source.meta(&one.0), b"{not json").unwrap();

    let report = run(&source, &bucket(), options()).await;

    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].kind, FailureKind::Corrupt);
}

#[tokio::test]
async fn a_file_larger_than_the_bound_is_failed_before_it_is_read_into_memory() {
    let big = file(T1, &[7u8; 2048], None);
    let small = file(T1, b"small", None);
    let source = Source::with(&[big.clone(), small.clone()]).await;
    let target = bucket();

    let report = run(
        &source,
        &target,
        Options {
            max_file_bytes: 1024,
            ..options()
        },
    )
    .await;

    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].key, big.0);
    assert_eq!(report.failed[0].kind, FailureKind::TooLarge);
    assert!(report.failed[0].reason.contains("maxFileBytes"));
    assert!(target.contains(&small.0) && !target.contains(&big.0));
}

#[tokio::test]
async fn a_file_that_is_already_in_the_bucket_is_skipped_and_left_as_it_is() {
    let kept = file(T1, b"already there", Some("theirs.txt"));
    let fresh = file(T1, b"not yet", None);
    let source = Source::with(&[kept.clone(), fresh.clone()]).await;
    let target = bucket();
    // the bucket holds the content under another file name: the content is the key, so it is the same file
    let other = ArtifactMeta::of("text/plain", Some("ours.txt".into()), b"already there");
    target.put(&kept.0, kept.2.clone(), &other).await.unwrap();

    let report = run(&source, &target, options()).await;

    assert_eq!((report.uploaded, report.skipped), (1, 1));
    let (found, _) = target.get(&kept.0).await.unwrap().unwrap();
    assert_eq!(
        found.filename.as_deref(),
        Some("ours.txt"),
        "not overwritten"
    );
}

#[tokio::test]
async fn an_object_of_the_wrong_size_under_the_key_is_replaced_and_counted() {
    let one = file(T1, b"the right content", None);
    let source = Source::with(std::slice::from_ref(&one)).await;
    let target = bucket().lenient();
    // what an interrupted or foreign upload left: another size under the key
    let put = target
        .presign_put(
            &one.0,
            &ArtifactMeta::of("text/plain", None, b"the right content"),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    target
        .upload(&put, Bytes::from_static(b"short"), jiff::Timestamp::now())
        .unwrap();

    let dry = run(
        &source,
        &target,
        Options {
            dry_run: true,
            ..options()
        },
    )
    .await;
    assert_eq!(
        (dry.would_upload, dry.replaced),
        (1, 1),
        "a dry run says it would replace"
    );
    let report = run(&source, &target, options()).await;

    assert_eq!(
        (report.uploaded, report.replaced, report.skipped),
        (1, 1, 0)
    );
    let (_, stream) = target.get(&one.0).await.unwrap().unwrap();
    let got: Vec<Bytes> = futures::TryStreamExt::try_collect(stream).await.unwrap();
    assert_eq!(got.concat(), b"the right content");
}

#[tokio::test]
async fn an_object_of_the_right_size_and_other_bytes_is_not_taken_for_the_file() {
    let one = file(T1, b"abcd", None);
    let source = Source::with(std::slice::from_ref(&one)).await;
    // a bucket that says no checksum: only reading the object back can tell
    let target = bucket().lenient().without_checksums();
    let put = target
        .presign_put(
            &one.0,
            &ArtifactMeta::of("text/plain", None, b"abcd"),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    target
        .upload(&put, Bytes::from_static(b"wxyz"), jiff::Timestamp::now())
        .unwrap();

    let report = run(&source, &target, options()).await;

    assert_eq!(
        (report.uploaded, report.replaced, report.skipped),
        (1, 1, 0)
    );
    let (_, stream) = target.get(&one.0).await.unwrap().unwrap();
    let got: Vec<Bytes> = futures::TryStreamExt::try_collect(stream).await.unwrap();
    assert_eq!(got.concat(), b"abcd");
}

#[tokio::test]
async fn a_bucket_that_does_not_say_its_checksums_is_read_back_to_skip_and_to_resume() {
    let files = [file(T1, b"one", None), file(T2, b"two", None)];
    let source = Source::with(&files).await;
    let target = bucket().without_checksums();
    run(&source, &target, options()).await;

    let again = run(&source, &target, options()).await;

    assert_eq!((again.uploaded, again.skipped), (0, 2));
}

#[tokio::test]
async fn a_store_that_cannot_say_what_it_holds_is_read_instead() {
    // the plain in-memory store answers `head` with "not supported", like the directory store
    let files = [file(T1, b"one", None)];
    let source = Source::with(&files).await;
    let target = MemoryArtifacts::new();

    let first = run(&source, &target, options()).await;
    let second = run(&source, &target, options()).await;

    assert_eq!((first.uploaded, first.skipped), (1, 0));
    assert_eq!((second.uploaded, second.skipped), (0, 1));
}

#[tokio::test]
async fn what_is_not_a_file_is_ignored_and_named_and_never_fails_the_run() {
    let one = file(T1, b"content", None);
    let source = Source::with(std::slice::from_ref(&one)).await;
    let dir = source
        .dir
        .path()
        .join("threads")
        .join(thread(T1).to_string());
    let hash = "a".repeat(64);
    std::fs::write(dir.join(".tmp-0190"), b"a crashed write").unwrap();
    std::fs::write(dir.join(&hash), b"bytes with no meta").unwrap();
    std::fs::write(dir.join(format!("{}.meta.json", "b".repeat(64))), b"{}").unwrap();
    std::fs::write(dir.join("notes.txt"), b"someone's note").unwrap();
    std::fs::write(source.dir.path().join("threads").join("stray"), b"x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", dir.join("c".repeat(64))).unwrap();

    let report = run(&source, &bucket(), options()).await;

    assert!(report.is_complete(), "{report:?}");
    assert_eq!((report.files, report.uploaded), (1, 1));
    let why = |why: IgnoredWhy| report.ignored.iter().filter(|i| i.why == why).count();
    assert_eq!(why(IgnoredWhy::Temporary), 1);
    assert_eq!(why(IgnoredWhy::NoMeta), 1);
    assert_eq!(why(IgnoredWhy::NoBytes), 1);
    assert_eq!(why(IgnoredWhy::Unrecognized), 2, "{:?}", report.ignored);
    #[cfg(unix)]
    assert_eq!(
        why(IgnoredWhy::NotARegularFile),
        1,
        "a link is not followed"
    );
}

#[tokio::test]
async fn an_empty_directory_and_one_with_no_threads_are_zero_files() {
    let dir = TempDir::new().unwrap();
    let store = FsArtifacts::open_read_only(dir.path()).await.unwrap();
    let report = migrate(&store, &bucket(), options(), |_, _| {})
        .await
        .unwrap();
    assert!(report.is_complete());
    assert_eq!(report.files, 0);
}

#[tokio::test]
async fn a_read_only_open_needs_an_existing_directory_and_creates_nothing() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("not-there");
    let err = FsArtifacts::open_read_only(&missing).await.unwrap_err();
    assert!(err.to_string().contains("not-there"), "{err}");
    assert!(!missing.exists(), "opening for reading makes no directory");

    let a_file = dir.path().join("a-file");
    std::fs::write(&a_file, b"x").unwrap();
    assert!(FsArtifacts::open_read_only(&a_file).await.is_err());

    FsArtifacts::open_read_only(dir.path()).await.unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "no probe file is written"
    );
}

#[tokio::test]
async fn nothing_is_deleted_from_either_side() {
    let kept = file(T1, b"kept", None);
    let source = Source::with(&[kept.clone(), file(T2, b"to go", None)]).await;
    let target = bucket();
    // something the bucket holds that the directory does not: it stays
    let unrelated = file(0x77, b"unrelated", None);
    target
        .put(&unrelated.0, unrelated.2.clone(), &unrelated.1)
        .await
        .unwrap();
    let before = source.listing();

    run(&source, &target, options()).await;
    run(&source, &target, options()).await;

    assert!(target.contains(&unrelated.0));
    assert_eq!(source.listing(), before);
}

/// A store that behaves as told: its failures are `Unavailable`, as a bucket that cannot be reached.
#[derive(Clone, Default)]
struct Wrapped {
    inner: MemoryArtifacts,
    /// Every call fails.
    down: bool,
    /// A `put` that succeeds and stores nothing.
    forgets: bool,
    calls: Arc<AtomicUsize>,
    puts: Arc<Mutex<Vec<ArtifactKey>>>,
}

impl Wrapped {
    fn new() -> Self {
        Wrapped {
            inner: MemoryArtifacts::presigning(),
            ..Wrapped::default()
        }
    }
}

impl ArtifactStore for Wrapped {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.down {
            return Err(ArtifactError::unavailable("the file could not be written"));
        }
        self.puts.lock().unwrap().push(*key);
        if self.forgets {
            return Ok(());
        }
        self.inner.put(key, bytes, meta).await
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        self.inner.get(key).await
    }

    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        self.inner.delete(key).await
    }

    async fn copy(&self, from: &ArtifactKey, to: &ArtifactKey) -> Result<(), ArtifactError> {
        self.inner.copy(from, to).await
    }

    async fn delete_prefix(&self, thread: ThreadId) -> Result<u64, ArtifactError> {
        self.inner.delete_prefix(thread).await
    }

    async fn presign_put(
        &self,
        key: &ArtifactKey,
        meta: &ArtifactMeta,
        ttl: Duration,
    ) -> Result<PresignedPut, ArtifactError> {
        self.inner.presign_put(key, meta, ttl).await
    }

    async fn head(&self, key: &ArtifactKey) -> Result<Option<ArtifactHead>, ArtifactError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.down {
            return Err(ArtifactError::unavailable("the file could not be read"));
        }
        self.inner.head(key).await
    }
}

#[tokio::test]
async fn a_put_that_the_bucket_does_not_hold_afterwards_is_failed_not_counted() {
    let one = file(T1, b"content", None);
    let source = Source::with(std::slice::from_ref(&one)).await;
    let target = Wrapped {
        forgets: true,
        ..Wrapped::new()
    };

    let report = run(&source, &target, options()).await;

    assert_eq!((report.uploaded, report.failed.len()), (0, 1));
    assert_eq!(report.failed[0].kind, FailureKind::Target);
    assert!(
        report.failed[0].reason.contains("after the put"),
        "{}",
        report.failed[0].reason
    );
}

#[tokio::test]
async fn a_bucket_that_cannot_be_reached_stops_the_run_after_a_few_failures() {
    let many: Vec<_> = (0..12u8).map(|n| file(T1, &[n; 4], None)).collect();
    let source = Source::with(&many).await;
    let target = Wrapped {
        down: true,
        ..Wrapped::new()
    };

    let report = run(&source, &target, options()).await;

    let tried = usize::try_from(MAX_CONSECUTIVE_TARGET_FAILURES).unwrap();
    assert_eq!(report.failed.len(), tried);
    assert!(report.failed.iter().all(|f| f.kind == FailureKind::Target));
    assert_eq!(report.not_attempted, (12 - tried) as u64);
    assert!(!report.is_complete());
    assert_eq!(report.files, 12);
    assert_eq!(
        target.calls.load(Ordering::SeqCst),
        tried,
        "after the stop the bucket is not asked again"
    );
}

#[tokio::test]
async fn a_failure_of_the_source_does_not_count_towards_stopping_the_run() {
    // more damaged files in a row than the limit on the bucket: the bucket is fine, so the run goes on
    let mut files = Vec::new();
    for n in 0..8u8 {
        files.push(file(T1, &[n; 4], None));
    }
    let good = file(T2, b"last and good", None);
    files.push(good.clone());
    let source = Source::with(&files).await;
    for (key, _, _) in &files[..8] {
        std::fs::write(source.blob(key), b"zzzz").unwrap();
    }
    let target = Wrapped::new();

    let report = run(&source, &target, options()).await;

    assert_eq!(
        (report.failed.len(), report.uploaded, report.not_attempted),
        (8, 1, 0)
    );
    assert_eq!(target.puts.lock().unwrap().as_slice(), [good.0]);
}
