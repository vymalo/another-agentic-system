//! An in-memory artifact store: the reference implementation of the conformance suite, and what a
//! test of the application puts in the bundle when it needs files to be kept.
//!
//! [`MemoryArtifacts::presigning`] is the same store **with the optional calls of the port**
//! ([`ArtifactStore::presign_put`], [`ArtifactStore::head`], ADR 0058): it signs nothing real (a URL
//! is `memory://bucket/<key>?ttl=<secs>`), and a test plays the agent and the bucket with
//! [`MemoryArtifacts::upload`]. Its knobs make the bucket behave the ways a real one might:
//! [`lenient`](MemoryArtifacts::lenient) takes bytes that do not match the signed checksum (what a
//! server that does not enforce it would do), and
//! [`without_checksums`](MemoryArtifacts::without_checksums) is a server that does not say the
//! checksum of an object. The plain store, [`MemoryArtifacts::new`], has none of this and answers
//! "not supported" like the directory store.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use jiff::Timestamp;
use orch_core::ThreadId;
use sha2::{Digest, Sha256};

use crate::{
    ArtifactError, ArtifactHead, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream,
    MAX_PRESIGN_TTL, PresignedPut, stream_of,
};

/// The header that carries the checksum the upload was signed for (S3's name, a hex value).
pub const CHECKSUM_HEADER: &str = "x-amz-checksum-sha256";
/// The header that carries the media type.
pub const CONTENT_TYPE_HEADER: &str = "content-type";
/// The header that carries the file name, percent-encoded as S3's user metadata is.
pub const FILENAME_HEADER: &str = "x-amz-meta-filename";

const URL_PREFIX: &str = "memory://bucket/";

#[derive(Debug, Clone)]
struct Stored {
    meta: ArtifactMeta,
    bytes: Bytes,
    /// When the bucket got it, for an upload; a plain `put` has no time.
    modified: Option<Timestamp>,
}

type Objects = HashMap<ArtifactKey, Stored>;

/// How a presigning store behaves.
#[derive(Debug, Clone, Copy)]
struct Presign {
    enforce_checksums: bool,
    report_checksums: bool,
}

/// The in-memory [`ArtifactStore`]. Cheap to clone: clones share the files, so a test keeps one to
/// look into while the application holds another.
#[derive(Debug, Clone, Default)]
pub struct MemoryArtifacts {
    objects: Arc<Mutex<Objects>>,
    presign: Option<Presign>,
}

impl MemoryArtifacts {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty store that can presign and say what it holds ([`ArtifactStore::can_presign`]); the
    /// bucket it plays refuses an upload whose bytes do not match the signed checksum, and says the
    /// checksum of an object.
    pub fn presigning() -> Self {
        MemoryArtifacts {
            objects: Arc::default(),
            presign: Some(Presign {
                enforce_checksums: true,
                report_checksums: true,
            }),
        }
    }

    /// The bucket takes bytes that do not match the signed checksum (a server that does not enforce
    /// it). No effect on a store that does not presign.
    #[must_use]
    pub fn lenient(mut self) -> Self {
        if let Some(presign) = &mut self.presign {
            presign.enforce_checksums = false;
        }
        self
    }

    /// The bucket does not say the checksum of an object ([`ArtifactHead::sha256`] is `None`). No
    /// effect on a store that does not presign.
    #[must_use]
    pub fn without_checksums(mut self) -> Self {
        if let Some(presign) = &mut self.presign {
            presign.report_checksums = false;
        }
        self
    }

    fn lock(&self) -> MutexGuard<'_, Objects> {
        // A test that panicked while holding the lock left a map that is still whole.
        self.objects.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many files are kept.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no file is kept.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Whether a file is kept under `key`.
    pub fn contains(&self, key: &ArtifactKey) -> bool {
        self.lock().contains_key(key)
    }

    /// Plays the agent's `PUT` and the bucket's answer: sends `bytes` the way `put` says, at `at`.
    /// The bucket checks that the request carries every signed header (the checksum, the type) and,
    /// unless [`lenient`](Self::lenient), that the bytes match the checksum; the object is then kept
    /// **as it came**, under the key the URL names, with the type and name of the headers.
    ///
    /// # Errors
    /// What the bucket would have answered, as text: a URL that is not this store's, a signed header
    /// missing, bytes that do not match the checksum.
    pub fn upload(&self, put: &PresignedPut, bytes: Bytes, at: Timestamp) -> Result<(), String> {
        let presign = self.presign.ok_or("this store does not presign")?;
        let path = put
            .url
            .strip_prefix(URL_PREFIX)
            .ok_or("the URL is not this bucket's")?;
        let key = ArtifactKey::parse(path.split('?').next().unwrap_or_default())
            .map_err(|e| e.to_string())?;
        let header = |name: &str| put.headers.get(name).map(String::as_str);
        let media_type = header(CONTENT_TYPE_HEADER).ok_or("the content-type header is missing")?;
        let signed = header(CHECKSUM_HEADER).ok_or("the checksum header is missing")?;
        if presign.enforce_checksums && signed != hex(&Sha256::digest(&bytes)) {
            return Err("BadDigest: the bytes do not match the signed checksum".to_owned());
        }
        let filename = header(FILENAME_HEADER).map(str::to_owned);
        let meta = ArtifactMeta {
            media_type: media_type.to_owned(),
            size: bytes.len() as u64,
            sha256: *key.sha256(),
            filename,
        };
        self.lock().insert(
            key,
            Stored {
                meta,
                bytes,
                modified: Some(at),
            },
        );
        Ok(())
    }
}

impl ArtifactStore for MemoryArtifacts {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        meta.check(key, &bytes)?;
        self.lock().insert(
            *key,
            Stored {
                meta: meta.clone(),
                bytes,
                modified: None,
            },
        );
        Ok(())
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        let found = self.lock().get(key).cloned();
        Ok(found.map(|stored| {
            let mut meta = stored.meta;
            meta.size = stored.bytes.len() as u64;
            (meta, stream_of(stored.bytes))
        }))
    }

    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        self.lock().remove(key);
        Ok(())
    }

    async fn copy(&self, from: &ArtifactKey, to: &ArtifactKey) -> Result<(), ArtifactError> {
        from.check_copy_to(to)?;
        let mut objects = self.lock();
        let found = objects.get(from).cloned().ok_or(ArtifactError::NotFound)?;
        objects.insert(*to, found);
        Ok(())
    }

    async fn delete_prefix(&self, thread: ThreadId) -> Result<u64, ArtifactError> {
        let mut objects = self.lock();
        let before = objects.len();
        objects.retain(|key, _| key.thread() != thread);
        Ok((before - objects.len()) as u64)
    }

    fn can_presign(&self) -> bool {
        self.presign.is_some()
    }

    async fn presign_put(
        &self,
        key: &ArtifactKey,
        meta: &ArtifactMeta,
        ttl: Duration,
    ) -> Result<PresignedPut, ArtifactError> {
        if self.presign.is_none() {
            return Err(ArtifactError::Unsupported);
        }
        meta.check_declared(key)?;
        if ttl.is_zero() || ttl > MAX_PRESIGN_TTL {
            return Err(ArtifactError::Invalid(
                "the lifetime of a presigned URL is from one second to seven days".to_owned(),
            ));
        }
        let mut headers = BTreeMap::from([
            (CONTENT_TYPE_HEADER.to_owned(), meta.media_type.clone()),
            (CHECKSUM_HEADER.to_owned(), hex(key.sha256())),
        ]);
        if let Some(name) = &meta.filename {
            headers.insert(FILENAME_HEADER.to_owned(), name.clone());
        }
        Ok(PresignedPut {
            url: format!("{URL_PREFIX}{key}?ttl={}", ttl.as_secs()),
            headers,
        })
    }

    async fn head(&self, key: &ArtifactKey) -> Result<Option<ArtifactHead>, ArtifactError> {
        let Some(presign) = self.presign else {
            return Err(ArtifactError::Unsupported);
        };
        Ok(self.lock().get(key).map(|stored| ArtifactHead {
            size: stored.bytes.len() as u64,
            media_type: Some(stored.meta.media_type.clone()),
            sha256: presign
                .report_checksums
                .then(|| Sha256::digest(&stored.bytes).into()),
            last_modified: stored.modified,
        }))
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}
