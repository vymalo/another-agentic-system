//! Downloads by a redirect to a presigned `GET` (ADR 0058 A1, `docs/api/uploads-v1.md`).
//!
//! The three file routes (the owner's, a signed-in reader's through a link, anybody's through a
//! public link) decide **who may read** a file here, once: [`App::readable_artifact`],
//! [`App::readable_shared_artifact`] and [`App::readable_public_artifact`] run exactly the checks
//! of the `open_*` calls (the permission `artifact.read`, the thread's owner or the link, the
//! thread's log naming the file, the same `NotFound` for every miss) and hand back a
//! [`ReadableFile`], which is the proof that they passed. What is done with it is the route's
//! choice: open the bytes ([`App::open_readable`]), or, when the deployment redirects
//! ([`DownloadMode`]) and the store can sign ([`App::redirect_ttl`]), read the file's meta without its
//! bytes ([`App::readable_meta`]) and sign a `GET` for it ([`App::presign_download`]).
//!
//! The URL is a credential for its seconds: nothing here logs it, and nothing that reaches the
//! event log holds it.

use std::time::Duration;

use orch_ports::{
    ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, Ports, PresignedGet,
    ResponseOverrides,
};

use super::App;
use crate::AppError;

/// The shortest `artifacts.s3.presignTtlSecs`.
const MIN_REDIRECT_TTL: Duration = Duration::from_secs(30);
/// The longest, an hour.
const MAX_REDIRECT_TTL: Duration = Duration::from_secs(3600);
/// The longest a URL for a reader of a share link lives: a revoked link ([ADR 0040]) must stop
/// working within a minute, and S3 has no revoke.
///
/// [ADR 0040]: ../../../../../docs/decisions/0040-thread-sharing-by-revocable-link.md
const LINK_REDIRECT_TTL: Duration = Duration::from_secs(60);

/// How the download routes send a file (`artifacts.s3.downloads`, ADR 0058 A1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DownloadMode {
    /// The bytes go through the orchestrator, streamed from the store. The default, and what a
    /// store that cannot sign always does.
    #[default]
    Stream,
    /// A `302` to a presigned `GET`, valid for `ttl` (at most a minute for a reader of a link).
    Redirect {
        /// How long the URL works.
        ttl: Duration,
    },
}

/// A lifetime a signature cannot carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a redirected download lives a whole number of seconds from 30 to 3600")]
pub struct DownloadModeError;

impl DownloadMode {
    /// A redirect whose URL works for `ttl`: whole seconds, 30 to 3600.
    ///
    /// # Errors
    /// [`DownloadModeError`] for any other lifetime.
    pub fn redirect(ttl: Duration) -> Result<Self, DownloadModeError> {
        if ttl.subsec_nanos() == 0 && (MIN_REDIRECT_TTL..=MAX_REDIRECT_TTL).contains(&ttl) {
            Ok(DownloadMode::Redirect { ttl })
        } else {
            Err(DownloadModeError)
        }
    }
}

/// Who is reading: the owner, or somebody who has a link to the thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reader {
    /// The thread's owner, through `GET /api/threads/{id}/artifacts/{sha}`.
    Owner,
    /// A reader of a share link, signed in or not.
    Link,
}

/// A file the caller may read: the checks of its route have passed (the permission, the thread's
/// owner or the link, the log naming the file), and the key is the thread's own. Only
/// [`App::readable_artifact`], [`App::readable_shared_artifact`] and
/// [`App::readable_public_artifact`] make one. It says nothing about whether the store holds the
/// file: opening or asking for its meta does.
#[derive(Debug, Clone, Copy)]
pub struct ReadableFile {
    key: ArtifactKey,
    reader: Reader,
}

impl ReadableFile {
    pub(super) fn new(key: ArtifactKey, reader: Reader) -> Self {
        ReadableFile { key, reader }
    }
}

impl<P: Ports> App<P> {
    /// The lifetime of the URL this file's download is redirected to, or `None` when it is
    /// streamed: the deployment did not ask for a redirect ([`DownloadMode::Stream`]), or the
    /// store cannot sign. A reader of a share link gets at most a minute.
    pub fn redirect_ttl(&self, file: &ReadableFile) -> Option<Duration> {
        let DownloadMode::Redirect { ttl } = self.cfg.downloads else {
            return None;
        };
        if !self.ports.artifacts().can_presign() {
            return None;
        }
        Some(match file.reader {
            Reader::Owner => ttl,
            Reader::Link => ttl.min(LINK_REDIRECT_TTL),
        })
    }

    /// What the store holds of the file without reading it: its type, size and name, for the
    /// headers a redirected download is signed with.
    ///
    /// # Errors
    /// [`AppError::NotFound`] for a file the store does not hold; [`AppError::Artifacts`] when the
    /// store fails, or holds an object that says no type.
    pub async fn readable_meta(&self, file: &ReadableFile) -> Result<ArtifactMeta, AppError> {
        let head = match self.ports.artifacts().head(&file.key).await {
            Ok(Some(head)) => head,
            Ok(None) | Err(ArtifactError::NotConfigured) => return Err(AppError::NotFound),
            // a store that can sign can say what it holds (the contract of `can_presign`)
            Err(ArtifactError::Unsupported) => {
                return Err(AppError::internal(
                    "the artifact store can sign a download but not say what a file is",
                ));
            }
            Err(e) => return Err(e.into()),
        };
        let media_type = head.media_type.ok_or_else(|| {
            AppError::Artifacts(ArtifactError::Corrupt(
                "the object has no content type".to_owned(),
            ))
        })?;
        Ok(ArtifactMeta {
            media_type,
            size: head.size,
            sha256: *file.key.sha256(),
            filename: head.filename,
        })
    }

    /// A presigned `GET` for the file, valid for `ttl`, that makes the answer carry `overrides`.
    /// Nothing is read: the store signs.
    ///
    /// # Errors
    /// [`AppError::Internal`] for a request the store refuses as wrong or cannot do (the route
    /// built the overrides and `redirect_ttl` said the store signs: either is a bug, never the
    /// caller's); [`AppError::Artifacts`] for the store's own failures.
    pub async fn presign_download(
        &self,
        file: &ReadableFile,
        overrides: &ResponseOverrides,
        ttl: Duration,
    ) -> Result<PresignedGet, AppError> {
        match self
            .ports
            .artifacts()
            .presign_get(&file.key, overrides, ttl)
            .await
        {
            Ok(get) => Ok(get),
            Err(e @ (ArtifactError::Invalid(_) | ArtifactError::Unsupported)) => Err(
                AppError::internal(format!("a download could not be signed: {e}")),
            ),
            Err(e) => Err(e.into()),
        }
    }

    /// The file's meta and its content as a stream, never held whole.
    ///
    /// # Errors
    /// [`AppError::NotFound`] for a file the store does not hold (or no store at all);
    /// [`AppError::Artifacts`] when the store fails.
    pub async fn open_readable(
        &self,
        file: &ReadableFile,
    ) -> Result<(ArtifactMeta, ByteStream), AppError> {
        self.fetch_key(&file.key).await
    }
}
