//! Moving the files of a directory store into another store (ADR 0058 A5.4, `orchestrator
//! artifacts migrate`): the volume of the directory store is read, each file is vetted, and the
//! ones the target does not hold are put there through [`ArtifactStore::put`], **under the same
//! keys**, so the references in the event log stay valid.
//!
//! ```text
//! scan the directory ──▶ for each file: read it, vet it ──▶ is it in the target? ──▶ skip
//!                                                                  │ no
//!                                                       put (or, on a dry run, say so)
//!                                                                  │
//!                                                      ask again: right size? ──▶ uploaded
//! ```
//!
//! **Vetting a file.** The directory's own read checks the meta against the key and the bytes
//! against the meta's size; this module then runs [`ArtifactMeta::check`] (the hash of the bytes is
//! the key's, the type and the name are text a header can carry). A file that fails is
//! [`Outcome::Failed`] and is **never** uploaded.
//!
//! **Idempotent and resumable.** A file the target already holds (right size and, when the
//! target says it or can be read back, the key's hash) is [`Outcome::Skipped`]: a second run
//! changes nothing, and a run that stopped half way is finished by running it again. An object of
//! another size or other bytes under the key is not the file: it is overwritten, counted as
//! `replaced`.
//!
//! **Memory.** [`ArtifactStore::put`] takes the whole file, so the migration holds one file at a
//! time, and refuses ([`FailureKind::TooLarge`]) one over [`Options::max_file_bytes`] before it
//! reads it. The ingest never kept a file over `artifacts.maxFileBytes`, which is that bound.
//!
//! **Read-only and never destructive.** The source is only read (open it with
//! [`FsArtifacts::open_read_only`] for a volume mounted read-only) and nothing is deleted from
//! either side.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures::StreamExt as _;
use orch_core::ThreadId;
use orch_ports::{ArtifactError, ArtifactKey, ArtifactStore};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    FsArtifacts, META_SUFFIX, OpenError, TEMP_PREFIX, THREADS_DIR, is_hash_name, not_found,
};

/// How many files in a row the target may fail on before the run stops: a bucket that cannot be
/// reached would otherwise cost every remaining file its retries and its timeout.
pub const MAX_CONSECUTIVE_TARGET_FAILURES: u32 = 5;

/// What a migration is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Read, vet and ask the target, but write nothing to it.
    pub dry_run: bool,
    /// The largest file taken (it is held whole in memory): `artifacts.maxFileBytes`.
    pub max_file_bytes: u64,
}

/// Why a file was not migrated. What the operator does about it differs: a damaged file needs a
/// person, a target that failed is tried again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FailureKind {
    /// The directory could not be read for this file (an I/O error, or it vanished).
    Unreadable,
    /// The file is damaged: its bytes do not hash to its key, are not the size its meta says, or
    /// its meta cannot be read or names another hash.
    Corrupt,
    /// The meta is not one a store keeps: [`ArtifactMeta::check`] refused its type or its name.
    Invalid,
    /// The file is over [`Options::max_file_bytes`].
    TooLarge,
    /// The target failed: unreachable, refused, or it does not hold the file after a put.
    Target,
}

impl FailureKind {
    /// The kind as the report writes it.
    pub const fn as_str(self) -> &'static str {
        match self {
            FailureKind::Unreadable => "unreadable",
            FailureKind::Corrupt => "corrupt",
            FailureKind::Invalid => "invalid",
            FailureKind::TooLarge => "tooLarge",
            FailureKind::Target => "target",
        }
    }
}

impl fmt::Display for FailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A file that is not in the target, and why. The reason names no path, URL or secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The file.
    pub key: ArtifactKey,
    /// What kind of trouble.
    pub kind: FailureKind,
    /// In words.
    pub reason: String,
}

/// What became of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// Put, and the target holds it with the right size afterwards.
    Uploaded {
        /// The file's size.
        bytes: u64,
        /// The target held something else under the key, which was overwritten.
        replaced: bool,
    },
    /// The target already held it.
    Skipped {
        /// The file's size.
        bytes: u64,
    },
    /// A dry run: it would be put.
    WouldUpload {
        /// The file's size.
        bytes: u64,
        /// The target holds something else under the key, which would be overwritten.
        replaced: bool,
    },
    /// Not in the target.
    Failed(Failure),
}

/// Why a name in the directory is not a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IgnoredWhy {
    /// A temporary file of a write that did not finish.
    Temporary,
    /// Bytes with no meta: what a delete in progress leaves, and the store finds no such file.
    NoMeta,
    /// A meta with no bytes: a write that did not finish, and the store finds no such file.
    NoBytes,
    /// A name the store writes, but not a regular file (a link or a directory): not followed.
    NotARegularFile,
    /// A name the store never writes.
    Unrecognized,
}

impl IgnoredWhy {
    /// The reason as the report writes it.
    pub const fn as_str(self) -> &'static str {
        match self {
            IgnoredWhy::Temporary => "a temporary file of a write that did not finish",
            IgnoredWhy::NoMeta => {
                "bytes with no meta (a delete in progress): not a file of the store"
            }
            IgnoredWhy::NoBytes => {
                "a meta with no bytes (a write that did not finish): not a file of the store"
            }
            IgnoredWhy::NotARegularFile => "not a regular file (a link or a directory)",
            IgnoredWhy::Unrecognized => "not a name this store writes",
        }
    }
}

impl fmt::Display for IgnoredWhy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An entry of the directory that is not a file of the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ignored {
    /// Under the root, such as `threads/<uuid>/<name>`.
    pub path: String,
    /// Why it is not a file.
    pub why: IgnoredWhy,
}

/// What the directory holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// The files (bytes and meta both there), in key order.
    pub keys: Vec<ArtifactKey>,
    /// Everything else.
    pub ignored: Vec<Ignored>,
}

/// The directory could not be read.
#[derive(Debug, thiserror::Error)]
#[error("cannot read the artifact directory {}: {source}", path.display())]
pub struct ScanError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

/// The result of a run. The counters add up: `files` is `uploaded + skipped + would_upload +
/// failed.len() + not_attempted`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Whether nothing was written.
    pub dry_run: bool,
    /// The files the directory holds.
    pub files: u64,
    /// Put and confirmed.
    pub uploaded: u64,
    /// Of `uploaded` and `would_upload`, those that overwrite or overwrote another object.
    pub replaced: u64,
    /// Already in the target.
    pub skipped: u64,
    /// A dry run: the files that would be put.
    pub would_upload: u64,
    /// The bytes of the files put.
    pub bytes_uploaded: u64,
    /// The bytes of the files the target already held.
    pub bytes_skipped: u64,
    /// A dry run: the bytes that would be put.
    pub bytes_would_upload: u64,
    /// The files that are not in the target, each with its reason.
    pub failed: Vec<Failure>,
    /// Files not tried because the run stopped ([`MAX_CONSECUTIVE_TARGET_FAILURES`]).
    pub not_attempted: u64,
    /// Entries of the directory that are not files of the store.
    pub ignored: Vec<Ignored>,
}

impl Report {
    /// Whether every file is (or, on a dry run, could be) in the target: none failed, none was
    /// left untried.
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty() && self.not_attempted == 0
    }
}

impl FsArtifacts {
    /// Opens the store at `root` **without writing anything**: the directory must exist and be
    /// readable; no directory is made and no probe file is written. For a volume mounted
    /// read-only. Calls that write fail like any write to a read-only disk
    /// ([`ArtifactError::Unavailable`]).
    ///
    /// # Errors
    /// [`OpenError`], naming the path and the I/O error.
    pub async fn open_read_only(root: impl Into<PathBuf>) -> Result<Self, OpenError> {
        let root = root.into();
        let fail = |source| OpenError {
            path: root.clone(),
            source,
        };
        let found = tokio::fs::metadata(&root).await.map_err(fail)?;
        if !found.is_dir() {
            return Err(fail(io::Error::from(io::ErrorKind::NotADirectory)));
        }
        // readable, not only there
        let _entries = tokio::fs::read_dir(&root).await.map_err(fail)?;
        Ok(FsArtifacts { root })
    }

    /// Lists the files of the directory: every `threads/<uuid>/<sha256>` that has its meta beside
    /// it, in key order, and what else is there. A root with no `threads` directory holds nothing.
    /// Nothing is followed (a link is ignored) and nothing is read but names.
    ///
    /// # Errors
    /// [`ScanError`] when a directory cannot be read.
    pub async fn scan(&self) -> Result<Scan, ScanError> {
        let threads = self.root.join(THREADS_DIR);
        let mut scan = Scan::default();
        let mut entries = match tokio::fs::read_dir(&threads).await {
            Ok(entries) => entries,
            Err(e) if not_found(&e) => return Ok(scan),
            Err(source) => return Err(scan_error(&threads, source)),
        };
        let mut found = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| scan_error(&threads, e))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry
                .file_type()
                .await
                .map_err(|e| scan_error(&entry.path(), e))?
                .is_dir();
            match thread_of(&name) {
                Some(thread) if is_dir => found.push((name, thread)),
                _ => scan.ignored.push(Ignored {
                    path: format!("{THREADS_DIR}/{name}"),
                    why: IgnoredWhy::Unrecognized,
                }),
            }
        }
        found.sort();
        for (name, thread) in found {
            self.scan_thread(&name, thread, &mut scan).await?;
        }
        scan.keys.sort();
        scan.ignored.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(scan)
    }

    async fn scan_thread(
        &self,
        name: &str,
        thread: ThreadId,
        scan: &mut Scan,
    ) -> Result<(), ScanError> {
        let dir = self.root.join(THREADS_DIR).join(name);
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| scan_error(&dir, e))?;
        let (mut blobs, mut metas) = (BTreeSet::new(), BTreeSet::new());
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| scan_error(&dir, e))?
        {
            let file = entry.file_name().to_string_lossy().into_owned();
            let regular = entry
                .file_type()
                .await
                .map_err(|e| scan_error(&entry.path(), e))?
                .is_file();
            let mut ignore = |why| {
                scan.ignored.push(Ignored {
                    path: format!("{THREADS_DIR}/{name}/{file}"),
                    why,
                });
            };
            if file.starts_with(TEMP_PREFIX) {
                ignore(IgnoredWhy::Temporary);
            } else if is_hash_name(&file) {
                if regular {
                    blobs.insert(file);
                } else {
                    ignore(IgnoredWhy::NotARegularFile);
                }
            } else if let Some(stem) = file.strip_suffix(META_SUFFIX).filter(|s| is_hash_name(s)) {
                if regular {
                    metas.insert(stem.to_owned());
                } else {
                    ignore(IgnoredWhy::NotARegularFile);
                }
            } else {
                ignore(IgnoredWhy::Unrecognized);
            }
        }
        for hash in &blobs {
            if metas.contains(hash) {
                if let Ok(key) = ArtifactKey::parse(&format!("{THREADS_DIR}/{name}/{hash}")) {
                    debug_assert_eq!(key.thread(), thread);
                    scan.keys.push(key);
                }
            } else {
                scan.ignored.push(Ignored {
                    path: format!("{THREADS_DIR}/{name}/{hash}"),
                    why: IgnoredWhy::NoMeta,
                });
            }
        }
        for hash in metas.difference(&blobs) {
            scan.ignored.push(Ignored {
                path: format!("{THREADS_DIR}/{name}/{hash}{META_SUFFIX}"),
                why: IgnoredWhy::NoBytes,
            });
        }
        Ok(())
    }
}

fn scan_error(path: &Path, source: io::Error) -> ScanError {
    ScanError {
        path: path.to_owned(),
        source,
    }
}

/// The thread a directory name is: the canonical text of a UUID (lower case, hyphenated).
fn thread_of(name: &str) -> Option<ThreadId> {
    let uuid = Uuid::parse_str(name).ok()?;
    (uuid.hyphenated().to_string() == name).then_some(ThreadId(uuid))
}

/// Puts the files of `from` into `to` that `to` does not hold. `observe` is told what became of each
/// file as it happens (a log line, a progress bar).
///
/// A file that cannot be migrated does not stop the run: it is in [`Report::failed`], and the
/// others go on. The run stops early only when the target fails on
/// [`MAX_CONSECUTIVE_TARGET_FAILURES`] files in a row ([`Report::not_attempted`]).
///
/// # Errors
/// [`ScanError`] when the directory cannot be listed; nothing is done then.
pub async fn migrate<D: ArtifactStore>(
    from: &FsArtifacts,
    to: &D,
    options: Options,
    mut observe: impl FnMut(&ArtifactKey, &Outcome),
) -> Result<Report, ScanError> {
    let scan = from.scan().await?;
    let mut report = Report {
        dry_run: options.dry_run,
        files: scan.keys.len() as u64,
        ignored: scan.ignored,
        ..Report::default()
    };
    let mut target_failures = 0u32;
    for (done, key) in scan.keys.iter().enumerate() {
        let outcome = migrate_one(from, to, key, options).await;
        observe(key, &outcome);
        match outcome {
            Outcome::Uploaded { bytes, replaced } => {
                report.uploaded += 1;
                report.bytes_uploaded += bytes;
                report.replaced += u64::from(replaced);
                target_failures = 0;
            }
            Outcome::Skipped { bytes } => {
                report.skipped += 1;
                report.bytes_skipped += bytes;
                target_failures = 0;
            }
            Outcome::WouldUpload { bytes, replaced } => {
                report.would_upload += 1;
                report.bytes_would_upload += bytes;
                report.replaced += u64::from(replaced);
                target_failures = 0;
            }
            Outcome::Failed(failure) => {
                // only the target's trouble says the target is down; a damaged file says nothing of it
                if failure.kind == FailureKind::Target {
                    target_failures += 1;
                }
                report.failed.push(failure);
            }
        }
        if target_failures >= MAX_CONSECUTIVE_TARGET_FAILURES {
            report.not_attempted = (scan.keys.len() - done - 1) as u64;
            break;
        }
    }
    Ok(report)
}

/// One file: read it, vet it, ask the target, put it, ask again.
async fn migrate_one<D: ArtifactStore>(
    from: &FsArtifacts,
    to: &D,
    key: &ArtifactKey,
    options: Options,
) -> Outcome {
    let fail = |kind, reason: String| {
        Outcome::Failed(Failure {
            key: *key,
            kind,
            reason,
        })
    };
    let (meta, mut stream) = match from.get(key).await {
        Ok(Some(found)) => found,
        Ok(None) => {
            return fail(
                FailureKind::Unreadable,
                "the file vanished while it was read".to_owned(),
            );
        }
        Err(e @ ArtifactError::Corrupt(_)) => return fail(FailureKind::Corrupt, e.to_string()),
        Err(e) => return fail(FailureKind::Unreadable, e.to_string()),
    };
    if meta.size > options.max_file_bytes {
        return fail(
            FailureKind::TooLarge,
            format!(
                "{} bytes: more than the {} bytes the migration holds in memory (artifacts.maxFileBytes)",
                meta.size, options.max_file_bytes
            ),
        );
    }
    let mut bytes = Vec::with_capacity(usize::try_from(meta.size).unwrap_or(0));
    while let Some(piece) = stream.next().await {
        match piece {
            Ok(piece) if (bytes.len() + piece.len()) as u64 <= meta.size => {
                bytes.extend_from_slice(&piece);
            }
            Ok(_) => {
                return fail(
                    FailureKind::Corrupt,
                    "the file is longer than its meta says".to_owned(),
                );
            }
            Err(e) => return fail(FailureKind::Unreadable, e.to_string()),
        }
    }
    let bytes = Bytes::from(bytes);
    if let Err(e) = meta.check(key, &bytes) {
        // `check` says Invalid for both; the bytes are the file's fault, the text is the meta's
        let damaged = bytes.len() as u64 != meta.size
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != *key.sha256();
        let kind = if damaged {
            FailureKind::Corrupt
        } else {
            FailureKind::Invalid
        };
        return fail(kind, e.to_string());
    }
    let size = meta.size;
    let replaced = match remote(to, key, size, true).await {
        Err(e) => {
            return fail(
                FailureKind::Target,
                format!("the target could not be asked: {e}"),
            );
        }
        Ok(Remote::Same) => return Outcome::Skipped { bytes: size },
        Ok(Remote::Absent) => false,
        Ok(Remote::Different(_)) => true,
    };
    if options.dry_run {
        return Outcome::WouldUpload {
            bytes: size,
            replaced,
        };
    }
    if let Err(e) = to.put(key, bytes, &meta).await {
        return fail(FailureKind::Target, format!("not written: {e}"));
    }
    match remote(to, key, size, false).await {
        Ok(Remote::Same) => Outcome::Uploaded {
            bytes: size,
            replaced,
        },
        Ok(Remote::Absent) => fail(
            FailureKind::Target,
            "not in the target after the put".to_owned(),
        ),
        Ok(Remote::Different(what)) => fail(
            FailureKind::Target,
            format!("the target holds {what} after the put"),
        ),
        Err(e) => fail(
            FailureKind::Target,
            format!("not confirmed after the put: {e}"),
        ),
    }
}

/// What the target holds under a key.
enum Remote {
    Absent,
    /// The file: the size and, when `deep`, the key's hash.
    Same,
    /// Something else, and what is different.
    Different(&'static str),
}

/// Asks the target about `key`. `head` first, which costs no read: a size that differs, or a
/// checksum that is not the key's, is not the file. With `deep`, an object whose checksum the target
/// does not say (or a target that cannot `head`) is read back and hashed; without it the size is
/// the proof (what is asked after a put the migration made with bytes it had hashed).
async fn remote<D: ArtifactStore>(
    to: &D,
    key: &ArtifactKey,
    size: u64,
    deep: bool,
) -> Result<Remote, ArtifactError> {
    match to.head(key).await {
        Ok(None) => Ok(Remote::Absent),
        Ok(Some(head)) if head.size != size => Ok(Remote::Different("another size")),
        Ok(Some(head)) => match head.sha256 {
            Some(sum) if sum == *key.sha256() => Ok(Remote::Same),
            Some(_) => Ok(Remote::Different("another checksum")),
            None if !deep => Ok(Remote::Same),
            None => read_back(to, key, size, true).await,
        },
        Err(ArtifactError::Corrupt(_)) => Ok(Remote::Different("another hash than its key")),
        Err(ArtifactError::Unsupported) => read_back(to, key, size, deep).await,
        Err(e) => Err(e),
    }
}

/// Reads the object under `key` from the target: its size, and with `hash` its bytes hashed as
/// they stream (the object is never held whole).
async fn read_back<D: ArtifactStore>(
    to: &D,
    key: &ArtifactKey,
    size: u64,
    hash: bool,
) -> Result<Remote, ArtifactError> {
    let found = match to.get(key).await {
        Ok(found) => found,
        Err(ArtifactError::Corrupt(_)) => {
            return Ok(Remote::Different("another hash than its key"));
        }
        Err(e) => return Err(e),
    };
    let Some((meta, mut stream)) = found else {
        return Ok(Remote::Absent);
    };
    if meta.size != size {
        return Ok(Remote::Different("another size"));
    }
    if !hash {
        return Ok(Remote::Same);
    }
    let (mut sum, mut seen) = (Sha256::new(), 0u64);
    while let Some(piece) = stream.next().await {
        let piece = piece?;
        seen += piece.len() as u64;
        sum.update(&piece);
    }
    if seen == size && <[u8; 32]>::from(sum.finalize()) == *key.sha256() {
        Ok(Remote::Same)
    } else {
        Ok(Remote::Different("other bytes"))
    }
}
