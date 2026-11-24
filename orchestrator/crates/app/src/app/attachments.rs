//! A person's own file, sent from a browser in the same origin (ADR 0067, wave 2 slice 12,
//! `docs/api/attachments.md`).
//!
//! The recommendation of ADR 0067's question 9 is what this implements: the bytes are checked
//! before anything is stored, kept in the artifact store by their own hash (ADR 0032) under the
//! thread's prefix, and read back through the file route that already exists — the thread's owner,
//! and nobody else (ADR 0039). No presigned upload is minted for a browser, and the browser never
//! holds a store credential.
//!
//! What is checked here is the whole rule: `thread.write` over the thread, a non-empty body, the
//! deployment's `artifacts.maxFileBytes`, one media type, and a file name that is text a header can
//! carry. The route only says **how** the bytes arrive; it decides neither size nor owner.

use bytes::Bytes;
use orch_core::ThreadId;
use orch_ports::{ArtifactError, ArtifactMeta, ArtifactStore, Ports};

use super::App;
use crate::Requester;
use crate::dispatcher::{canonical_media_type, clean_filename, sniff};
use crate::error::AppError;

/// Why a person's file was refused (ADR 0067, slice 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AttachmentRefusal {
    /// The body is over the deployment's `artifacts.maxFileBytes`.
    #[error("the file is larger than {max} bytes")]
    TooLarge {
        /// The most bytes this deployment keeps.
        max: u64,
    },
    /// The body is empty: an empty file is not kept.
    #[error("the file is empty")]
    Empty,
    /// `Content-Type` is not one `type/subtype`.
    #[error("the media type is not one type/subtype")]
    BadMediaType,
    /// `Content-Disposition`'s name does not parse, or is not usable.
    #[error("the file name is not usable")]
    BadFilename,
    /// This deployment has no artifact store.
    #[error("this deployment keeps no files")]
    NoStore,
}

impl AttachmentRefusal {
    /// The stable name a client acts on (`docs/api/attachments.md`, the refusals table).
    pub fn code(&self) -> &'static str {
        match self {
            AttachmentRefusal::TooLarge { .. } => "too_large",
            AttachmentRefusal::Empty => "empty_body",
            AttachmentRefusal::BadMediaType => "bad_media_type",
            AttachmentRefusal::BadFilename => "bad_filename",
            AttachmentRefusal::NoStore => "attachments_unavailable",
        }
    }
}

impl From<AttachmentRefusal> for AppError {
    fn from(refusal: AttachmentRefusal) -> Self {
        AppError::Attachment(refusal)
    }
}

/// A file a person sent: what the store keeps and what a client reads back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttachment {
    /// The content's hash, 64 lower-case hex digits: the file's name in its thread.
    pub sha256: String,
    /// The bytes stored.
    pub size: u64,
    /// The type the file is kept and served as.
    pub media_type: String,
    /// The cleaned name, when one was given.
    pub filename: Option<String>,
}

/// The most a browser may send in one file (ADR 0067, question 9): a ceiling the deployment's
/// `artifacts.maxFileBytes` may lower and never raise.
pub const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;

impl<P: Ports> App<P> {
    /// The most bytes one attachment may have in this deployment.
    pub fn attachment_limit(&self) -> u64 {
        self.file_limits().max_file_bytes.min(MAX_ATTACHMENT_BYTES)
    }

    /// Keeps a person's file in `thread` and answers its reference (`POST
    /// /api/threads/{threadId}/attachments`, ADR 0067 slice 12).
    ///
    /// In this order: the permission and the thread are [`writable_thread`](Self::writable_thread)'s
    /// (a foreign thread is `NotFound`, ADR 0039), the body is not empty and not over the
    /// deployment's `artifacts.maxFileBytes` — **before anything is put** — the declared type is one
    /// media type, and the name is usable. The content is then hashed and put under
    /// `threads/{thread}/{sha256}`; a second put of the same bytes writes the same object
    /// (ADR 0032).
    ///
    /// # Errors
    /// [`AppError::NotFound`], [`AppError::Forbidden`]; [`AppError::Attachment`] for a size or a
    /// type or a name the rule refuses, and for a deployment with no store; [`AppError::Artifacts`]
    /// when the store fails.
    pub async fn store_attachment(
        &self,
        who: &impl Requester,
        thread: ThreadId,
        filename: Option<&str>,
        media_type: Option<&str>,
        bytes: &[u8],
    ) -> Result<StoredAttachment, AppError> {
        let access = self.access(who);
        self.writable_thread(&access, thread).await?;
        let max = self.attachment_limit();
        if bytes.is_empty() {
            return Err(AttachmentRefusal::Empty.into());
        }
        if bytes.len() as u64 > max {
            return Err(AttachmentRefusal::TooLarge { max }.into());
        }
        let filename = match filename {
            Some(raw) => Some(clean_filename(raw).ok_or(AttachmentRefusal::BadFilename)?),
            None => None,
        };
        let declared = match media_type {
            Some(declared) => {
                Some(canonical_media_type(declared).ok_or(AttachmentRefusal::BadMediaType)?)
            }
            None => None,
        };
        let media_type = sniff(declared.as_deref(), filename.as_deref(), bytes);
        let meta = ArtifactMeta::of(media_type.clone(), filename.clone(), bytes);
        let key = meta.key(thread);
        match self
            .ports
            .artifacts()
            .put(&key, Bytes::copy_from_slice(bytes), &meta)
            .await
        {
            Ok(()) => {}
            Err(ArtifactError::NotConfigured) => return Err(AttachmentRefusal::NoStore.into()),
            Err(error) => return Err(error.into()),
        }
        Ok(StoredAttachment {
            sha256: key.sha256_hex(),
            size: meta.size,
            media_type,
            filename,
        })
    }
}
