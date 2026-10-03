//! The artifact store port: where the files an agent hands over are kept (ADR 0032, ADR 0009).
//!
//! The event log never holds a file. It holds a reference (`threads/<thread>/<sha256>`, the
//! content's own hash), and the bytes live here, in a directory ([`orch-artifacts-fs`]) or in an
//! S3-compatible bucket ([`orch-artifacts-s3`]). The orchestrator reasons on the log only; a store
//! is durable like git is (ADR 0003), and a deployment without one ([`NoArtifacts`]) refuses a file
//! with "no artifact store configured" and keeps the rest of the chat working.
//!
//! The key is the content's hash, so the port is **idempotent by content**: the same bytes put
//! twice under one key are one object, and a retried put (a worker that died after the store
//! accepted the file and before the log did) changes nothing. A store never accepts bytes that do
//! not hash to their key ([`ArtifactMeta::check`]). A file is read as a **stream**
//! ([`ByteStream`]), so serving a large file never holds it whole; a file is written whole (it is
//! at most `artifacts.maxFileBytes`, 10 MiB by default, and already in memory when the worker
//! checked it).
//!
//! [`orch-artifacts-fs`]: ../../artifacts-fs/README.md
//! [`orch-artifacts-s3`]: ../../artifacts-s3/README.md

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use bytes::Bytes;
use futures::Stream;
use orch_core::{BoxError, Classify, ErrorClass, ThreadId};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The size of the pieces a [`ByteStream`] of an in-memory file is cut into, 64 KiB. A store may
/// cut its own differently; a consumer takes whatever sizes come.
pub const CHUNK_BYTES: usize = 64 * 1024;

/// The longest media type or file name a store takes, in bytes (the limit of a file name on most
/// file systems, and well inside an HTTP header).
pub const MAX_NAME_BYTES: usize = 255;

/// The address of a file: the thread it was shared in and the SHA-256 of its content,
/// `threads/<thread uuid>/<64 lowercase hex digits>`.
///
/// It is built from its two parts or parsed from its one canonical text, never from a free string,
/// so a key can never hold a `..`, a separator or a name a file system or a bucket reads in a way
/// the orchestrator did not mean: whatever a store maps it to is made of a UUID and hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtifactKey {
    thread: ThreadId,
    sha256: [u8; 32],
}

impl ArtifactKey {
    /// The key of the file with this hash in this thread.
    pub const fn new(thread: ThreadId, sha256: [u8; 32]) -> Self {
        ArtifactKey { thread, sha256 }
    }

    /// The thread the file was shared in.
    pub const fn thread(&self) -> ThreadId {
        self.thread
    }

    /// The SHA-256 of the content.
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// The SHA-256 of the content as 64 lowercase hexadecimal digits (what the API's URL carries).
    pub fn sha256_hex(&self) -> String {
        hex(&self.sha256)
    }

    /// Checks what every store checks before it copies `self` to `to`: a copy keeps the content,
    /// so the two keys hold one hash.
    ///
    /// # Errors
    /// [`ArtifactError::Invalid`] when the hashes differ.
    pub fn check_copy_to(&self, to: &ArtifactKey) -> Result<(), ArtifactError> {
        if self.sha256 == to.sha256 {
            Ok(())
        } else {
            Err(ArtifactError::Invalid(
                "a copy keeps the content: the two keys hold different hashes".to_owned(),
            ))
        }
    }

    /// Reads the canonical text of a key.
    ///
    /// # Errors
    /// [`ArtifactError::InvalidKey`] for anything else: another prefix, more or fewer segments,
    /// a UUID that is not lowercase and hyphenated, a hash that is not 64 lowercase hex digits.
    pub fn parse(text: &str) -> Result<Self, ArtifactError> {
        let invalid =
            || ArtifactError::InvalidKey("expected threads/<uuid>/<sha256 in hex>".into());
        let mut parts = text.split('/');
        let (Some("threads"), Some(thread), Some(hash), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        let uuid = Uuid::parse_str(thread).map_err(|_| invalid())?;
        // `Uuid::parse_str` also reads upper case, braces and URNs; only the canonical text is a key.
        if uuid.hyphenated().to_string() != thread {
            return Err(invalid());
        }
        Ok(ArtifactKey {
            thread: ThreadId(uuid),
            sha256: unhex(hash).ok_or_else(invalid)?,
        })
    }
}

impl fmt::Display for ArtifactKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "threads/{}/{}", self.thread, self.sha256_hex())
    }
}

impl fmt::Debug for ArtifactKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ArtifactKey({self})")
    }
}

impl FromStr for ArtifactKey {
    type Err = ArtifactError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        ArtifactKey::parse(text)
    }
}

/// What a store keeps beside the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactMeta {
    /// The media type, sniffed by the worker (`image/png`; `application/octet-stream` when unsure).
    pub media_type: String,
    /// The size in bytes.
    pub size: u64,
    /// The SHA-256 of the content: the second half of the key.
    pub sha256: [u8; 32],
    /// The file's name as the agent gave it, if it did. Untrusted text: a server that puts it in a
    /// header sanitizes it first.
    pub filename: Option<String>,
}

impl ArtifactMeta {
    /// The meta of `bytes`: its size and hash, with the media type and file name given.
    pub fn of(media_type: impl Into<String>, filename: Option<String>, bytes: &[u8]) -> Self {
        ArtifactMeta {
            media_type: media_type.into(),
            size: bytes.len() as u64,
            sha256: Sha256::digest(bytes).into(),
            filename,
        }
    }

    /// The key of this content in `thread`.
    pub const fn key(&self, thread: ThreadId) -> ArtifactKey {
        ArtifactKey::new(thread, self.sha256)
    }

    /// Checks what every store must check before it keeps anything: `meta` is the description of
    /// `bytes` (the size and the hash are the content's own), `key` is where that content goes
    /// (its hash is the key's), and the media type and file name are text a header can carry.
    ///
    /// # Errors
    /// [`ArtifactError::Invalid`], naming what disagrees (never the content).
    pub fn check(&self, key: &ArtifactKey, bytes: &[u8]) -> Result<(), ArtifactError> {
        let invalid = |what: &str| Err(ArtifactError::Invalid(what.to_owned()));
        if self.size != bytes.len() as u64 {
            return invalid("the size in the meta is not the size of the bytes");
        }
        if &self.sha256 != key.sha256() {
            return invalid("the hash in the meta is not the hash in the key");
        }
        if &<[u8; 32]>::from(Sha256::digest(bytes)) != key.sha256() {
            return invalid("the bytes do not hash to the key");
        }
        if !is_header_text(&self.media_type, true) {
            return invalid("the media type is empty, too long or not plain text");
        }
        if let Some(name) = &self.filename
            && !is_header_text(name, false)
        {
            return invalid("the file name is empty, too long or has a control character");
        }
        Ok(())
    }
}

/// Text that is safe to keep in a header or a file's metadata: not empty, at most
/// [`MAX_NAME_BYTES`], no control character. A media type is also visible ASCII.
fn is_header_text(text: &str, ascii: bool) -> bool {
    !text.is_empty()
        && text.len() <= MAX_NAME_BYTES
        && text
            .chars()
            .all(|c| !c.is_control() && (!ascii || (c.is_ascii_graphic() || c == ' ')))
}

/// A file as a stream of pieces. A piece that fails ends the stream: the reader got a prefix and
/// must not take it for the file.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, ArtifactError>> + Send>>;

/// `bytes` as a [`ByteStream`] of pieces of [`CHUNK_BYTES`] (slices of the one allocation, no
/// copy). For a store that holds a file in memory, and for a test.
pub fn stream_of(bytes: Bytes) -> ByteStream {
    let pieces = futures::stream::unfold(bytes, |mut rest| async move {
        if rest.is_empty() {
            None
        } else {
            let piece = rest.split_to(rest.len().min(CHUNK_BYTES));
            Some((Ok(piece), rest))
        }
    });
    Box::pin(pieces)
}

/// Where the files of threads are kept.
///
/// An implementation keeps them durably, never reads or writes outside its own root, and never
/// puts a credential into an error. It does not enforce a size limit (the worker does, before it
/// calls), and it does not decide who may read a file (the API does, by the thread's owner).
pub trait ArtifactStore: Send + Sync + 'static {
    /// Keeps `bytes` under `key`, with `meta` beside them.
    ///
    /// **Idempotent by content**: the key holds the content's hash, so a file that is already
    /// there is the same file, and putting it again succeeds and changes nothing. (The `meta` of a
    /// second put may differ in the file name; one of the two is kept, whole.) Concurrent puts of
    /// one key all succeed and leave one whole object. When this returns `Ok` the file is durable
    /// and `get` finds it.
    ///
    /// # Errors
    /// [`ArtifactError::Invalid`] when `bytes` do not hash to `key` or `meta` does not describe
    /// them ([`ArtifactMeta::check`]); the store's own errors otherwise.
    fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> impl Future<Output = Result<(), ArtifactError>> + Send;

    /// The meta of the file under `key` and its content as a stream; `None` when there is none.
    ///
    /// # Errors
    /// [`ArtifactError`]; a failure while the stream is read is an `Err` item that ends it.
    fn get(
        &self,
        key: &ArtifactKey,
    ) -> impl Future<Output = Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError>> + Send;

    /// Removes the file under `key`. Removing what is not there succeeds.
    ///
    /// # Errors
    /// [`ArtifactError`] when the store cannot be reached.
    fn delete(&self, key: &ArtifactKey) -> impl Future<Output = Result<(), ArtifactError>> + Send;

    /// Makes the file under `from` also the file under `to`, with its meta, without the bytes
    /// passing through the caller (a fork gives its own thread's key to what it inherits, ADR 0043).
    /// The two keys hold one hash: a copy does not change content.
    ///
    /// **Idempotent**: copying onto a key that already holds this file succeeds and changes
    /// nothing. When it returns `Ok`, `to` is durable and `get` finds it, and it is the file's own:
    /// deleting `from` afterwards leaves it. It reads `from` only within the store's own root and
    /// checks the content's hash where the store can (a store that links or copies on the server
    /// has the key's hash and the size to go by, and says so).
    ///
    /// # Errors
    /// [`ArtifactError::Invalid`] when the two keys do not hold the same hash;
    /// [`ArtifactError::NotFound`] when there is no file under `from`; the store's own errors
    /// otherwise ([`ArtifactError::NotConfigured`] for [`NoArtifacts`]).
    fn copy(
        &self,
        from: &ArtifactKey,
        to: &ArtifactKey,
    ) -> impl Future<Output = Result<(), ArtifactError>> + Send;

    /// Removes every file of `thread`, and no file of another thread (even one that holds the same
    /// hash): the erasure of a deleted thread's files (ADR 0043). Returns how many files it
    /// removed.
    ///
    /// **Idempotent**: a thread with no file left is `Ok(0)`, so the inline purge of a delete and
    /// the sweep that finishes one that failed may both run. When it returns `Ok`, `get` finds no
    /// file of the thread. A file put while it runs may survive it; the caller deletes the
    /// thread's row first, so nothing puts a file for it afterwards. It touches nothing outside
    /// the thread's own part of the store's root.
    ///
    /// # Errors
    /// [`ArtifactError`] when the store cannot be reached ([`ArtifactError::NotConfigured`] for
    /// [`NoArtifacts`]: a deployment with no store has no file to remove, which the caller
    /// knows).
    fn delete_prefix(
        &self,
        thread: ThreadId,
    ) -> impl Future<Output = Result<u64, ArtifactError>> + Send;
}

/// Why a store did not do what it was asked.
///
/// `Unavailable`'s `source` is the transport or I/O error (adapters box theirs: ADR 0009). It may
/// hold a path or a URL, so it never reaches the chat log; none of them ever holds a credential.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ArtifactError {
    /// The text is not the canonical form of a key.
    #[error("not an artifact key: {0}")]
    InvalidKey(String),
    /// The request is wrong: the bytes do not hash to the key, the meta does not describe them, a
    /// name is not plain text.
    #[error("invalid artifact: {0}")]
    Invalid(String),
    /// The store could not be reached or failed (a full disk, a refused connection, a 5xx).
    /// Retryable.
    #[error("artifact store unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without paths, URLs or secrets.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// The store refused the credential, or wants one it was not given.
    #[error("the artifact store refused the credentials")]
    Unauthenticated,
    /// What the store holds is not what it should: a file whose size is not its meta's, a meta
    /// that cannot be read. Alert.
    #[error("the artifact store holds corrupt data: {0}")]
    Corrupt(String),
    /// There is no file under the key of a [`copy`](ArtifactStore::copy)'s source. (A `get` of a
    /// missing key is `Ok(None)`, not this.)
    #[error("no such artifact")]
    NotFound,
    /// This deployment has no store ([`NoArtifacts`]).
    #[error("no artifact store configured")]
    NotConfigured,
}

impl ArtifactError {
    /// The store failed or could not be reached.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        ArtifactError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause of an `Unavailable` error; any other variant is returned
    /// unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            ArtifactError::Unavailable { detail, .. } => ArtifactError::Unavailable {
                detail,
                source: Some(source.into()),
            },
            other @ (ArtifactError::InvalidKey(_)
            | ArtifactError::Invalid(_)
            | ArtifactError::Unauthenticated
            | ArtifactError::Corrupt(_)
            | ArtifactError::NotFound
            | ArtifactError::NotConfigured) => other,
        }
    }
}

impl Classify for ArtifactError {
    fn class(&self) -> ErrorClass {
        match self {
            ArtifactError::InvalidKey(_) | ArtifactError::Invalid(_) => ErrorClass::Invalid,
            ArtifactError::Unavailable { .. } => ErrorClass::Transient,
            ArtifactError::Unauthenticated => ErrorClass::Unauthenticated,
            ArtifactError::Corrupt(_) => ErrorClass::Corrupt,
            ArtifactError::NotFound => ErrorClass::NotFound,
            ArtifactError::NotConfigured => ErrorClass::Unsupported,
        }
    }
}

/// The store of a deployment that has none: every call is [`ArtifactError::NotConfigured`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoArtifacts;

impl ArtifactStore for NoArtifacts {
    async fn put(
        &self,
        _key: &ArtifactKey,
        _bytes: Bytes,
        _meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        Err(ArtifactError::NotConfigured)
    }

    async fn get(
        &self,
        _key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        Err(ArtifactError::NotConfigured)
    }

    async fn delete(&self, _key: &ArtifactKey) -> Result<(), ArtifactError> {
        Err(ArtifactError::NotConfigured)
    }

    async fn copy(&self, _from: &ArtifactKey, _to: &ArtifactKey) -> Result<(), ArtifactError> {
        Err(ArtifactError::NotConfigured)
    }

    async fn delete_prefix(&self, _thread: ThreadId) -> Result<u64, ArtifactError> {
        Err(ArtifactError::NotConfigured)
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0xf)]));
    }
    out
}

/// 64 lowercase hex digits, or `None`.
fn unhex(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let value = |d: u8| match d {
        b'0'..=b'9' => Some(d - b'0'),
        b'a'..=b'f' => Some(d - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(digits.chunks_exact(2)) {
        *slot = value(pair[0])? << 4 | value(pair[1])?;
    }
    Some(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use futures::StreamExt as _;

    use super::*;

    fn thread() -> ThreadId {
        ThreadId(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001))
    }

    #[test]
    fn a_key_is_the_thread_and_the_hash_and_reads_back() {
        let meta = ArtifactMeta::of("text/plain", None, b"hello");
        let key = meta.key(thread());
        let text = key.to_string();
        assert_eq!(
            text,
            "threads/01900000-0000-7000-8000-000000000001/\
             2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(ArtifactKey::parse(&text).unwrap(), key);
        assert_eq!(text.parse::<ArtifactKey>().unwrap(), key);
        assert_eq!(key.thread(), thread());
        assert_eq!(key.sha256_hex(), &text[text.len() - 64..]);
        assert_eq!(format!("{key:?}"), format!("ArtifactKey({text})"));
    }

    #[test]
    fn only_the_canonical_text_is_a_key() {
        let hash = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let uuid = "01900000-0000-7000-8000-000000000001";
        let bad = [
            String::new(),
            "threads".to_owned(),
            format!("threads/{uuid}"),
            format!("threads/{uuid}/{hash}/"),
            format!("threads/{uuid}/{hash}/extra"),
            format!("/threads/{uuid}/{hash}"),
            format!("thread/{uuid}/{hash}"),
            format!("threads/../{uuid}/{hash}"),
            format!("threads/{uuid}/../{hash}"),
            format!("threads/{uuid}/..{}", &hash[2..]),
            format!("threads/{uuid}/{}", &hash[..63]),
            format!("threads/{uuid}/{hash}0"),
            format!("threads/{uuid}/{}", hash.to_uppercase()),
            format!("threads/{}/{hash}", "0190000A-0000-7000-8000-00000000000B"),
            format!("threads/{}/{hash}", uuid.replace('-', "")),
            format!("threads/{{{uuid}}}/{hash}"),
            format!("threads/urn:uuid:{uuid}/{hash}"),
            format!("threads//{hash}"),
            format!("threads/{uuid}/%2e%2e"),
            format!("threads/{uuid}\\{hash}"),
            format!("threads/{uuid}/{hash}\0"),
            format!("threads/{uuid}/{}g", &hash[1..]),
        ];
        for text in bad {
            let err = ArtifactKey::parse(&text).unwrap_err();
            assert!(matches!(err, ArtifactError::InvalidKey(_)), "{text:?}");
            assert_eq!(err.class(), ErrorClass::Invalid);
        }
    }

    #[test]
    fn the_meta_of_bytes_is_checked_against_the_key_and_the_bytes() {
        let bytes = b"some content";
        let meta = ArtifactMeta::of("text/plain", Some("notes.txt".into()), bytes);
        let key = meta.key(thread());
        assert_eq!(meta.size, 12);
        meta.check(&key, bytes).unwrap();

        let refused = |meta: &ArtifactMeta, key: &ArtifactKey, bytes: &[u8]| {
            matches!(meta.check(key, bytes), Err(ArtifactError::Invalid(_)))
        };
        // other bytes of the same length, and of another length
        assert!(refused(&meta, &key, b"other  bytes"));
        assert!(refused(&meta, &key, b"short"));
        // a key of other content
        let other = ArtifactMeta::of("text/plain", None, b"other").key(thread());
        assert!(refused(&meta, &other, bytes));
        // a meta that lies about its size or its hash
        let mut lie = meta.clone();
        lie.size = 13;
        assert!(refused(&lie, &key, bytes));
        let mut lie = meta.clone();
        lie.sha256[0] ^= 1;
        assert!(refused(&lie, &key, bytes));
        // names that are not plain text
        for media_type in [
            "",
            "image/png\r\nX-Evil: 1",
            "text/plain\u{7f}",
            "tëxt/plain",
        ] {
            let mut bad = meta.clone();
            bad.media_type = media_type.to_owned();
            assert!(refused(&bad, &key, bytes), "{media_type:?}");
        }
        for name in ["", "a\nb", "a\0b", &"x".repeat(MAX_NAME_BYTES + 1)] {
            let mut bad = meta.clone();
            bad.filename = Some(name.to_owned());
            assert!(refused(&bad, &key, bytes), "{name:?}");
        }
        // a name with spaces, quotes and other scripts is fine
        let mut ok = meta.clone();
        ok.filename = Some("Résumé \"final\" 日本語.txt".to_owned());
        ok.check(&key, bytes).unwrap();
        // and so is the empty file
        let empty = ArtifactMeta::of("application/octet-stream", None, b"");
        empty.check(&empty.key(thread()), b"").unwrap();
    }

    #[tokio::test]
    async fn bytes_stream_in_pieces_that_join_to_the_whole() {
        let bytes = Bytes::from(vec![7u8; CHUNK_BYTES * 2 + 5]);
        let pieces: Vec<_> = stream_of(bytes.clone()).collect().await;
        assert_eq!(pieces.len(), 3);
        let joined: Vec<u8> = pieces
            .into_iter()
            .flat_map(|p| p.unwrap().to_vec())
            .collect();
        assert_eq!(joined, bytes);
        assert!(stream_of(Bytes::new()).next().await.is_none());
    }

    #[tokio::test]
    async fn no_artifacts_refuses_every_call_and_says_why() {
        let meta = ArtifactMeta::of("text/plain", None, b"x");
        let key = meta.key(thread());
        let put = NoArtifacts
            .put(&key, Bytes::from_static(b"x"), &meta)
            .await
            .unwrap_err();
        let get = NoArtifacts.get(&key).await.err().unwrap();
        let delete = NoArtifacts.delete(&key).await.unwrap_err();
        let copy = NoArtifacts.copy(&key, &key).await.unwrap_err();
        let purge = NoArtifacts.delete_prefix(thread()).await.unwrap_err();
        for err in [put, get, delete, copy, purge] {
            assert!(matches!(err, ArtifactError::NotConfigured), "{err}");
            assert_eq!(err.to_string(), "no artifact store configured");
            assert_eq!(err.class(), ErrorClass::Unsupported);
            assert!(!err.is_retryable());
        }
    }

    #[test]
    fn a_copy_keeps_the_hash() {
        let meta = ArtifactMeta::of("text/plain", None, b"x");
        let other = ArtifactMeta::of("text/plain", None, b"y");
        let from = meta.key(thread());
        assert!(
            from.check_copy_to(&meta.key(ThreadId(Uuid::from_u128(9))))
                .is_ok()
        );
        assert!(matches!(
            from.check_copy_to(&other.key(thread())),
            Err(ArtifactError::Invalid(_))
        ));
    }

    #[test]
    fn every_error_has_the_class_that_says_what_to_do() {
        let cases: [(ArtifactError, ErrorClass); 7] = [
            (ArtifactError::InvalidKey("x".into()), ErrorClass::Invalid),
            (ArtifactError::Invalid("x".into()), ErrorClass::Invalid),
            (ArtifactError::unavailable("down"), ErrorClass::Transient),
            (ArtifactError::Unauthenticated, ErrorClass::Unauthenticated),
            (ArtifactError::Corrupt("x".into()), ErrorClass::Corrupt),
            (ArtifactError::NotFound, ErrorClass::NotFound),
            (ArtifactError::NotConfigured, ErrorClass::Unsupported),
        ];
        for (err, class) in cases {
            assert_eq!(err.class(), class, "{err}");
        }
        assert!(ArtifactError::unavailable("x").is_retryable());
        assert!(ArtifactError::Corrupt("x".into()).class().should_alert());
        let with = ArtifactError::unavailable("x").with_source(std::io::Error::other("disk"));
        assert!(std::error::Error::source(&with).is_some());
        // a source is kept only where one belongs
        let kept = ArtifactError::Unauthenticated.with_source(std::io::Error::other("disk"));
        assert!(matches!(kept, ArtifactError::Unauthenticated));
    }
}
