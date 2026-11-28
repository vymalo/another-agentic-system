//! A person's memory notes and settings, and the port that keeps them (ADR 0091, ADR 0009).
//!
//! The rules of what a note is, what a read returns and what a grant means are
//! `orch-core`'s `memory` module (`docs/api/memory-v1.md`); this is only where they are kept. A note and a
//! person's grants belong to **one person**: no event of the log records either, the orchestrator
//! never reads one to reason, and they are never copied into a thread, a fork or an export.
//!
//! # The name is [`MemoryNoteStore`], not `MemoryStore`
//!
//! `orch_ports::memory::MemoryStore` is the in-memory *adapter* of the thread store (behind the
//! `testkit` feature). A port called `MemoryStore` would read as that type and be mistaken for it at
//! every import.
//!
//! # Not a supertrait of [`ThreadStore`](crate::ThreadStore)
//!
//! Unlike [`PromptStore`](crate::PromptStore) and [`TeamPresetStore`](crate::TeamPresetStore), this
//! port is **standalone**: making it a supertrait would break every implementer of `ThreadStore` (the
//! SQLite store and the test double of `orch-app`) for slices that do not read memory yet. Adapters
//! that share the thread store's database (`MemoryStore`, `PgStore`) implement it beside their
//! threads; wiring it into the application's `PortSet` belongs to the slice that adds the routes.
//!
//! # Who sees what, decided by the store
//!
//! **The owner is an argument of every method and is in every statement of an adapter**: a note
//! that is not the caller's is not found, and the answer is the same as for an id that never
//! existed (`forget_note` answers `false`, never "not yours"). An id is unique **per person**, so
//! saving an id that another person holds is allowed and touches nothing of theirs. There is no edit:
//! `memory-v1` has `memory_save` and `memory_forget` only, and a save never replaces a note.
//!
//! # Off by default
//!
//! A person with no row reads as [`MemorySettings::default`]: no agent has a grant. Setting a grant
//! of `false`, `false` is allowed and reads back as an entry of that agent, which is the person
//! having touched it.
//!
//! # A save is checked in this order
//!
//! [`Exists`](MemoryNoteStoreError::Exists) (the person already holds that id), then
//! [`TooMany`](MemoryNoteStoreError::TooMany) ([`MAX_NOTES`] held; never an eviction).

use std::future::Future;

use orch_core::{BoxError, Classify, ErrorClass, Timestamp, UserId};

use orch_core::{ForgetInput, Grant, MemorySettings, Note, ReadInput, ReadResult, SaveInput};
pub use orch_core::{MAX_NOTES, MAX_READ_NOTES};

/// Why a store refused or failed. None holds a note's words or an owner.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MemoryNoteStoreError {
    /// The person already holds [`MAX_NOTES`]. Nothing was written.
    #[error("a person holds at most {limit} notes")]
    TooMany {
        /// The limit.
        limit: usize,
    },
    /// The person already holds a note with that id. Nothing was written.
    #[error("a note with that id exists")]
    Exists,
    /// The store could not be reached or failed.
    #[error("the memory store is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without words of a note.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// What the store holds is not a note. Alert.
    #[error("the memory store holds corrupt data: {0}")]
    Corrupt(String),
    /// The store refused a request that no retry will change (a value the database cannot hold, a
    /// statement it rejects): a bug to fix, never a reason to wait.
    /// [`Unavailable`](Self::Unavailable) is the one a caller retries.
    #[error("the memory store refused the request: {detail}")]
    Internal {
        /// What was attempted, without words of a note.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
}

impl MemoryNoteStoreError {
    /// The store failed or could not be reached.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        MemoryNoteStoreError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// The store refused a request that no retry will change.
    pub fn internal(detail: impl Into<String>) -> Self {
        MemoryNoteStoreError::Internal {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause of an `Unavailable` or `Internal` error; any other variant is
    /// returned unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            MemoryNoteStoreError::Unavailable { detail, .. } => MemoryNoteStoreError::Unavailable {
                detail,
                source: Some(source.into()),
            },
            MemoryNoteStoreError::Internal { detail, .. } => MemoryNoteStoreError::Internal {
                detail,
                source: Some(source.into()),
            },
            other @ (MemoryNoteStoreError::TooMany { .. }
            | MemoryNoteStoreError::Exists
            | MemoryNoteStoreError::Corrupt(_)) => other,
        }
    }
}

impl Classify for MemoryNoteStoreError {
    fn class(&self) -> ErrorClass {
        match self {
            MemoryNoteStoreError::TooMany { .. } | MemoryNoteStoreError::Exists => {
                ErrorClass::Rejected
            }
            MemoryNoteStoreError::Unavailable { .. } => ErrorClass::Transient,
            MemoryNoteStoreError::Corrupt(_) => ErrorClass::Corrupt,
            MemoryNoteStoreError::Internal { .. } => ErrorClass::Internal,
        }
    }
}

/// Every person's memory: their notes and their per-agent grants.
///
/// The methods carry `_note` or `_memory` so that they never collide with the other ports a store
/// implements beside this one (`get`, `list` and `delete` are taken).
pub trait MemoryNoteStore: Send + Sync + 'static {
    /// Stores `text` as a note of `owner` at once (there is no approval step), written by
    /// `written_by` (an agent id, or `person`) at `at`, under `id` (the caller makes it: `m-` and
    /// an opaque suffix). Refused in the order [`Exists`](MemoryNoteStoreError::Exists),
    /// [`TooMany`](MemoryNoteStoreError::TooMany); the count and the insert are one decision, so
    /// writers racing for the last place take turns. Answers the note as a read returns it.
    fn save_note(
        &self,
        owner: &UserId,
        text: &SaveInput,
        written_by: &str,
        id: &str,
        at: Timestamp,
    ) -> impl Future<Output = Result<Note, MemoryNoteStoreError>> + Send;

    /// What `memory_read` answers for `owner`: the notes whose text holds `input.query`
    /// (case-insensitive substring, no wildcard), **newest first**, cut at `input.limit` clamped to
    /// `1..=MAX_READ_NOTES`. `total` counts the matches before the cut. Notes saved at the same
    /// instant are ordered by the order they were saved in, the later first.
    fn read_notes(
        &self,
        owner: &UserId,
        input: &ReadInput,
    ) -> impl Future<Output = Result<ReadResult, MemoryNoteStoreError>> + Send;

    /// Every note of `owner`, newest first, at most [`MAX_NOTES`]: the person's own page.
    fn list_notes(
        &self,
        owner: &UserId,
    ) -> impl Future<Output = Result<Vec<Note>, MemoryNoteStoreError>> + Send;

    /// Deletes `owner`'s note `id`. `true` when it was there; **`false` for an id that is not
    /// there, that was already deleted, or that is another person's** (the same answer, so a retry
    /// is safe and nothing is revealed).
    fn forget_note(
        &self,
        owner: &UserId,
        id: &ForgetInput,
    ) -> impl Future<Output = Result<bool, MemoryNoteStoreError>> + Send;

    /// The grants of `owner`: [`MemorySettings::default`] (both `false` for everyone) for a person
    /// who never touched them.
    fn memory_settings(
        &self,
        owner: &UserId,
    ) -> impl Future<Output = Result<MemorySettings, MemoryNoteStoreError>> + Send;

    /// Sets what `owner` lets `agent` do (replacing the earlier grant of that agent). `agent` is
    /// not empty.
    fn set_memory_grant(
        &self,
        owner: &UserId,
        agent: &str,
        grant: Grant,
    ) -> impl Future<Output = Result<(), MemoryNoteStoreError>> + Send;
}
