//! [`ArtifactStore`] over a directory (ADR 0032, ADR 0009): the store for development and for a
//! single node. Files are kept under one root, which every role of the deployment must share (a
//! control plane and a worker on different machines need the S3 store, [`orch-artifacts-s3`]).
//!
//! ```text
//! <root>/threads/<thread uuid>/<sha256>             the bytes, mode 0600
//! <root>/threads/<thread uuid>/<sha256>.meta.json   the meta, mode 0600
//! ```
//!
//! **A write is atomic.** The meta and then the bytes are each written to a temporary file in the
//! same directory (`.tmp-<uuid>`), flushed and fsynced, and renamed into place; the directory is
//! fsynced after. The bytes come last, so a file is found only when both are whole: a crash in the
//! middle leaves a meta without bytes (not found, and a retried put overwrites it) or a temporary
//! file nobody reads (safe to delete when no process is writing).
//!
//! **A copy is a hard link** when the two keys are on one file system (they are: one root), so no
//! byte moves, and the two names are independent (a file is never written in place, and a delete
//! removes a name). Where a link is refused the bytes are copied to a temporary file, hashed as
//! they are read, and renamed. The meta goes first and the bytes last, as in a put. A copy reads
//! only the two regular files of its source key, and follows no link.
//!
//! **A delete of a thread's files removes the directory** `<root>/threads/<thread uuid>`
//! ([`ArtifactStore::delete_prefix`], the erasure of ADR 0043): one `remove_dir_all` of a path built
//! from the UUID alone, so it can reach no other thread's files nor anything outside the root. A
//! thread with no directory is `Ok(0)`.
//!
//! **A key cannot leave the root.** A key is a thread UUID and 64 hex digits
//! ([`ArtifactKey`]): the path is built from those alone, never from text of an agent, so there is
//! no `..`, separator or absolute path to escape with. The root itself is trusted: a symbolic link
//! placed under it by someone who can write to it is followed.
//!
//! [`orch-artifacts-s3`]: ../../artifacts-s3/README.md

use std::io;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures::StreamExt as _;
use orch_core::ThreadId;
use orch_ports::{
    ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, CHUNK_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

/// The directory under the root that holds every thread's files.
const THREADS_DIR: &str = "threads";

/// The suffix of the file that holds a file's meta.
const META_SUFFIX: &str = ".meta.json";

/// The prefix of a file being written.
const TEMP_PREFIX: &str = ".tmp-";

/// The root could not be used.
#[derive(Debug, thiserror::Error)]
#[error("cannot use {} as the artifact root: {source}", path.display())]
pub struct OpenError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

/// The store: a directory.
#[derive(Debug, Clone)]
pub struct FsArtifacts {
    root: PathBuf,
}

impl FsArtifacts {
    /// Opens the store at `root`: creates the directory (mode 0700) if it is not there, and checks
    /// that a file can be written in it, so a root that cannot be used is found at startup and not
    /// when the first file is shared.
    ///
    /// # Errors
    /// [`OpenError`], naming the path and the I/O error.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, OpenError> {
        let root = root.into();
        let fail = |source| OpenError {
            path: root.clone(),
            source,
        };
        create_dirs(&root).await.map_err(fail)?;
        let probe = root.join(format!("{TEMP_PREFIX}{}", Uuid::now_v7()));
        let mut file = create_file(&probe).await.map_err(fail)?;
        let written = file.write_all(b"ok").await;
        drop(file);
        let removed = tokio::fs::remove_file(&probe).await;
        written.and(removed).map_err(fail)?;
        Ok(FsArtifacts { root })
    }

    /// The directory the store keeps its files in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory of a thread's files.
    fn dir_of(&self, key: &ArtifactKey) -> PathBuf {
        self.root.join(THREADS_DIR).join(key.thread().to_string())
    }

    /// The file that holds the bytes, and the file that holds the meta.
    fn paths_of(&self, key: &ArtifactKey) -> (PathBuf, PathBuf) {
        let dir = self.dir_of(key);
        let name = key.sha256_hex();
        (dir.join(&name), dir.join(format!("{name}{META_SUFFIX}")))
    }
}

/// What the meta file holds. The key has the hash; the size is read off the bytes' own file and
/// checked against this.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Sidecar {
    media_type: String,
    size: u64,
    sha256: String,
    filename: Option<String>,
}

impl Sidecar {
    fn of(meta: &ArtifactMeta, key: &ArtifactKey) -> Self {
        Sidecar {
            media_type: meta.media_type.clone(),
            size: meta.size,
            sha256: key.sha256_hex(),
            filename: meta.filename.clone(),
        }
    }

    fn into_meta(self, key: &ArtifactKey) -> Result<ArtifactMeta, ArtifactError> {
        if self.sha256 != key.sha256_hex() {
            return Err(ArtifactError::Corrupt(
                "the meta file names another hash than its key".into(),
            ));
        }
        Ok(ArtifactMeta {
            media_type: self.media_type,
            size: self.size,
            sha256: *key.sha256(),
            filename: self.filename,
        })
    }
}

fn io_error(detail: &str, source: io::Error) -> ArtifactError {
    ArtifactError::unavailable(detail).with_source(source)
}

/// Creates `path` and its parents, readable by the owner only.
async fn create_dirs(path: &Path) -> io::Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path).await
}

/// Creates a new file, readable and writable by the owner only. It must not exist.
async fn create_file(path: &Path) -> io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).await
}

/// Writes `bytes` to `dir/name`: a temporary file beside it, fsynced, then renamed over it.
async fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let temp = dir.join(format!("{TEMP_PREFIX}{}", Uuid::now_v7()));
    let written = async {
        let mut file = create_file(&temp).await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temp, dir.join(name)).await
    }
    .await;
    if written.is_err() {
        // best effort: the error that matters is the one that is returned
        let _ = tokio::fs::remove_file(&temp).await;
    }
    written
}

/// Puts the file `src` at `dir/name`, whole or not at all: a hard link to a temporary name when the
/// two are on one file system (no bytes move, and the two names are then independent: removing one
/// leaves the other, and a file is never written in place), else a copy that is hashed as it is
/// read and compared with `sha256`; then a rename.
async fn link_or_copy(
    src: &Path,
    dir: &Path,
    name: &str,
    sha256: &[u8; 32],
) -> Result<(), ArtifactError> {
    place(src, dir, name, sha256, true).await
}

/// [`link_or_copy`], with the link left out when `link` is false (what a refused link does).
async fn place(
    src: &Path,
    dir: &Path,
    name: &str,
    sha256: &[u8; 32],
    link: bool,
) -> Result<(), ArtifactError> {
    let temp = dir.join(format!("{TEMP_PREFIX}{}", Uuid::now_v7()));
    let done = async {
        if !link || tokio::fs::hard_link(src, &temp).await.is_err() {
            copy_hashed(src, &temp, sha256).await?;
        }
        tokio::fs::rename(&temp, dir.join(name))
            .await
            .map_err(|e| io_error("the file could not be copied", e))
    }
    .await;
    if done.is_err() {
        // best effort: the error that matters is the one that is returned
        let _ = tokio::fs::remove_file(&temp).await;
    }
    done
}

/// Copies `src` to the new file `temp`, fsynced, and checks that what was read hashes to `sha256`.
async fn copy_hashed(src: &Path, temp: &Path, sha256: &[u8; 32]) -> Result<(), ArtifactError> {
    let fail = |source| io_error("the file could not be copied", source);
    let mut from = tokio::fs::File::open(src).await.map_err(fail)?;
    let mut to = create_file(temp).await.map_err(fail)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_BYTES];
    loop {
        let read = from.read(&mut buffer).await.map_err(fail)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        to.write_all(&buffer[..read]).await.map_err(fail)?;
    }
    to.flush().await.map_err(fail)?;
    to.sync_all().await.map_err(fail)?;
    if &<[u8; 32]>::from(hash.finalize()) != sha256 {
        return Err(ArtifactError::Corrupt(
            "the file does not hash to its key".into(),
        ));
    }
    Ok(())
}

/// Makes the renames in `dir` durable.
async fn sync_dir(dir: &Path) -> io::Result<()> {
    tokio::fs::File::open(dir).await?.sync_all().await
}

fn not_found(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}

impl ArtifactStore for FsArtifacts {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        meta.check(key, &bytes)?;
        let (blob, sidecar) = self.paths_of(key);
        let dir = self.dir_of(key);
        let fail = |source| io_error("the file could not be written", source);

        let wanted = serde_json::to_vec(&Sidecar::of(meta, key))
            .map_err(|e| ArtifactError::Invalid(format!("the meta cannot be written: {e}")))?;
        create_dirs(&dir).await.map_err(fail)?;

        // The bytes are there already (the key is their hash): only the meta may differ.
        let kept = tokio::fs::metadata(&blob)
            .await
            .is_ok_and(|m| m.is_file() && m.len() == meta.size);
        let same_meta = kept
            && tokio::fs::read(&sidecar)
                .await
                .is_ok_and(|found| found == wanted);
        if same_meta {
            return Ok(());
        }
        let meta_name = format!("{}{META_SUFFIX}", key.sha256_hex());
        write_atomic(&dir, &meta_name, &wanted)
            .await
            .map_err(fail)?;
        if !kept {
            write_atomic(&dir, &key.sha256_hex(), &bytes)
                .await
                .map_err(fail)?;
        }
        sync_dir(&dir).await.map_err(fail)
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        let (blob, sidecar) = self.paths_of(key);
        let fail = |source| io_error("the file could not be read", source);
        let file = match tokio::fs::File::open(&blob).await {
            Ok(file) => file,
            Err(e) if not_found(&e) => return Ok(None),
            Err(e) => return Err(fail(e)),
        };
        let text = match tokio::fs::read(&sidecar).await {
            Ok(text) => text,
            // bytes with no meta are what a delete in progress leaves
            Err(e) if not_found(&e) => return Ok(None),
            Err(e) => return Err(fail(e)),
        };
        let meta = serde_json::from_slice::<Sidecar>(&text)
            .map_err(|_| ArtifactError::Corrupt("the meta file cannot be read".into()))?
            .into_meta(key)?;
        let size = file.metadata().await.map_err(fail)?.len();
        if size != meta.size {
            return Err(ArtifactError::Corrupt(
                "the file is not the size its meta says".into(),
            ));
        }
        let pieces = ReaderStream::with_capacity(file, CHUNK_BYTES)
            .map(|piece| piece.map_err(|e| io_error("the file could not be read", e)));
        Ok(Some((meta, Box::pin(pieces))))
    }

    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        let (blob, sidecar) = self.paths_of(key);
        // The bytes first: a file is found only when both are there.
        for path in [blob, sidecar] {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(e) if not_found(&e) => {}
                Err(e) => return Err(io_error("the file could not be removed", e)),
            }
        }
        Ok(())
    }

    async fn copy(&self, from: &ArtifactKey, to: &ArtifactKey) -> Result<(), ArtifactError> {
        from.check_copy_to(to)?;
        let (src_blob, src_sidecar) = self.paths_of(from);
        let (dst_blob, dst_sidecar) = self.paths_of(to);
        let dir = self.dir_of(to);
        let fail = |source| io_error("the file could not be copied", source);

        // The source is two regular files of our own root: a link placed in their stead is not
        // followed (the root is trusted, but a copy never reads outside it by a link it did not make).
        for path in [&src_blob, &src_sidecar] {
            match tokio::fs::symlink_metadata(path).await {
                Ok(found) if found.file_type().is_file() => {}
                Ok(_) => return Err(ArtifactError::Corrupt("not a regular file".into())),
                Err(e) if not_found(&e) => return Err(ArtifactError::NotFound),
                Err(e) => return Err(fail(e)),
            }
        }
        let text = match tokio::fs::read(&src_sidecar).await {
            Ok(text) => text,
            Err(e) if not_found(&e) => return Err(ArtifactError::NotFound),
            Err(e) => return Err(fail(e)),
        };
        let meta = serde_json::from_slice::<Sidecar>(&text)
            .map_err(|_| ArtifactError::Corrupt("the meta file cannot be read".into()))?
            .into_meta(from)?;
        let size = tokio::fs::symlink_metadata(&src_blob)
            .await
            .map_err(fail)?
            .len();
        if size != meta.size {
            return Err(ArtifactError::Corrupt(
                "the file is not the size its meta says".into(),
            ));
        }
        if from == to {
            return Ok(());
        }

        create_dirs(&dir).await.map_err(fail)?;
        // The hash is the key's, so the sidecar is the source's word for word.
        let kept = tokio::fs::metadata(&dst_blob)
            .await
            .is_ok_and(|m| m.is_file() && m.len() == meta.size);
        if kept
            && tokio::fs::read(&dst_sidecar)
                .await
                .is_ok_and(|found| found == text)
        {
            return Ok(());
        }
        // The meta, then the bytes, as a put writes them: a file is found only when both are whole.
        let meta_name = format!("{}{META_SUFFIX}", to.sha256_hex());
        write_atomic(&dir, &meta_name, &text).await.map_err(fail)?;
        if !kept {
            link_or_copy(&src_blob, &dir, &to.sha256_hex(), to.sha256()).await?;
        }
        sync_dir(&dir).await.map_err(fail)
    }

    async fn delete_prefix(&self, thread: ThreadId) -> Result<u64, ArtifactError> {
        let dir = self.root.join(THREADS_DIR).join(thread.to_string());
        let fail = |source| io_error("the thread's files could not be removed", source);
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            // no directory is no file: the thread had none, or an earlier purge removed them
            Err(e) if not_found(&e) => return Ok(0),
            Err(e) => return Err(fail(e)),
        };
        // The files are the entries named by a hash; a meta, and a temporary file that a crashed
        // put left, go with the directory but are not files to count.
        let mut files = 0;
        while let Some(entry) = entries.next_entry().await.map_err(fail)? {
            if is_hash_name(&entry.file_name().to_string_lossy()) {
                files += 1;
            }
        }
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => {}
            Err(e) if not_found(&e) => {}
            Err(e) => return Err(fail(e)),
        }
        // the removal of the directory is durable once its parent says so
        let parent = self.root.join(THREADS_DIR);
        match sync_dir(&parent).await {
            Ok(()) => {}
            Err(e) if not_found(&e) => {}
            Err(e) => return Err(fail(e)),
        }
        Ok(files)
    }
}

/// Whether `name` is the file name of a file's bytes: 64 lower-case hex digits.
fn is_hash_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Component;

    use super::*;

    fn key_of(thread: u128, content: &[u8]) -> ArtifactKey {
        ArtifactMeta::of("text/plain", None, content).key(ThreadId(Uuid::from_u128(thread)))
    }

    /// Every component of every path the store builds is a plain name under the root: no `..`, no
    /// root, no prefix, whatever the key's two parts are.
    #[test]
    fn a_key_maps_to_a_path_under_the_root_and_nowhere_else() {
        let root = PathBuf::from("/srv/artifacts");
        let store = FsArtifacts { root: root.clone() };
        for (thread, hash) in [
            (0u128, [0u8; 32]),
            (u128::MAX, [0xffu8; 32]),
            (0x2e2e, [0x2e; 32]),
        ] {
            let key = ArtifactKey::new(ThreadId(Uuid::from_u128(thread)), hash);
            let (blob, sidecar) = store.paths_of(&key);
            for path in [blob, sidecar, store.dir_of(&key)] {
                let relative = path.strip_prefix(&root).expect("under the root");
                assert!(
                    relative
                        .components()
                        .all(|c| matches!(c, Component::Normal(_))),
                    "{path:?}"
                );
                let depth = relative.components().count();
                assert!((2..=3).contains(&depth), "{path:?}");
            }
        }
    }

    /// The text a person or an agent could try is never a key, so it never reaches a path.
    #[test]
    fn traversal_attempts_are_not_keys() {
        let hash = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let uuid = "01900000-0000-7000-8000-000000000001";
        for text in [
            format!("threads/../../etc/passwd/{hash}"),
            format!("threads/{uuid}/../../../etc/passwd"),
            format!("threads/{uuid}/{hash}/../../x"),
            "/etc/passwd".to_owned(),
            format!("threads/{uuid}/..%2f..%2f{}", &hash[12..]),
            format!("threads\\..\\{uuid}\\{hash}"),
            format!("threads/{uuid}/{hash}.meta.json"),
            format!("threads/{uuid}/.tmp-x"),
            format!("threads/{uuid}/{hash}\u{0}../x"),
        ] {
            assert!(ArtifactKey::parse(&text).is_err(), "{text:?}");
        }
        // and a key that is valid is made of hex digits and a UUID alone
        let key = key_of(1, b"x");
        assert!(
            key.to_string().bytes().all(|b| b.is_ascii_hexdigit()
                || b == b'-'
                || b == b'/'
                || b"threads".contains(&b))
        );
    }

    /// The path that is taken where a hard link is refused (another file system): the bytes are
    /// copied, hashed as they are read, and a file that does not hash to its key is not kept.
    #[tokio::test]
    async fn the_copy_that_stands_in_for_a_link_checks_the_hash() {
        let held = tempfile::TempDir::new().unwrap();
        let dir = held.path().to_path_buf();
        let src = dir.join("src");
        tokio::fs::write(&src, b"the content").await.unwrap();
        let hash: [u8; 32] = Sha256::digest(b"the content").into();

        place(&src, &dir, "ok", &hash, false).await.unwrap();
        assert_eq!(
            tokio::fs::read(dir.join("ok")).await.unwrap(),
            b"the content"
        );

        let wrong: [u8; 32] = Sha256::digest(b"another").into();
        let err = place(&src, &dir, "bad", &wrong, false).await.unwrap_err();
        assert!(matches!(err, ArtifactError::Corrupt(_)), "{err}");
        assert!(!dir.join("bad").exists());
        let mut left = tokio::fs::read_dir(&dir).await.unwrap();
        while let Some(entry) = left.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(!name.starts_with(TEMP_PREFIX), "{name}");
        }
    }

    #[test]
    fn the_sidecar_of_another_hash_is_corrupt() {
        let key = key_of(1, b"a");
        let other = key_of(1, b"b");
        let sidecar = Sidecar::of(&ArtifactMeta::of("text/plain", None, b"b"), &other);
        assert!(matches!(
            sidecar.into_meta(&key),
            Err(ArtifactError::Corrupt(_))
        ));
    }
}
