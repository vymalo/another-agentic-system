//! A person's prompt library and the port that keeps it (ADR 0070, ADR 0009).
//!
//! A person saves prompts (a name, a title, a body with `{{variables}}`, the variables as
//! questions of the UI catalog) and may share each with roles. No event of the log records one:
//! the library is the person's, never folded by the core and never copied into a thread, a fork or
//! an export. The rules of what a prompt is are [`orch_core::prompt`]; what is kept here is
//! the definitions, per person, and who may see which.
//!
//! # The port is a supertrait of [`ThreadStore`](crate::ThreadStore)
//!
//! **Every `ThreadStore` is a [`PromptStore`]: an adapter for another database implements it beside
//! its threads.** The library lives in the thread store's database and pool, and a port of its own
//! would need a new associated type and a new field in every `PortSet` literal. This is a breaking
//! change for every implementer of `ThreadStore` (three in this workspace: `MemoryStore`,
//! `PgStore` and the test double of `orch-app`).
//!
//! # Who sees what, decided by the store
//!
//! The visibility rule lives behind the port, on a [`PromptViewer`] (the caller and the roles claim
//! of their credential, as given): a prompt is seen by its owner and by a viewer whom its audience
//! admits ([`PromptAudience::admits`]: `"*"`, or one of their roles, byte for byte, **no
//! administrator exception**). **No type of the port carries an owner**, and a prompt that is not
//! the viewer's comes back with a private audience and `own == false`, so no handler can name the
//! author or the roles a prompt was shared with. Anything else is `None` or [`NotFound`]: the same
//! answer as for an id that never existed.
//!
//! [`NotFound`]: PromptStoreError::NotFound
//!
//! # Writes
//!
//! Only the owner writes. Every write that changes an existing prompt is a **compare and swap** on a
//! [`PromptRevision`] the store keeps and a read returns: a stale one is
//! [`Conflict`](PromptStoreError::Conflict), an absent one is [`NotFound`](PromptStoreError::NotFound)
//! (a deleted prompt stays deleted), and only [`insert_prompt`](PromptStore::insert_prompt)
//! creates. A revision is a number of one sequence, **never given twice**, a delete included. At most
//! [`MAX_PROMPTS`] per owner, **enforced by the store**; the refusals of an insert are decided in the
//! order [`Exists`](PromptStoreError::Exists), [`NameTaken`](PromptStoreError::NameTaken),
//! [`TooMany`](PromptStoreError::TooMany), so every adapter refuses the same way.

use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;

use orch_core::prompt::{PromptAudience, PromptDefinition, PromptId};
use orch_core::{BoxError, Classify, ErrorClass, Timestamp, UserId};

pub use orch_core::prompt::{MAX_PROMPTS, PAGE as PROMPT_PAGE};

/// Who is asking: the person, and the roles claim of their credential **as given** (not the roles
/// that `auth.roles` defines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptViewer {
    /// The person: the key of what they own.
    pub user: UserId,
    /// The values of their roles claim.
    pub roles: BTreeSet<String>,
}

/// Which prompts a listing holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptScope {
    /// The viewer's own.
    Mine,
    /// The ones shared with the viewer, **not** their own.
    Shared,
    /// Both.
    All,
}

/// The revision of a prompt: a number of one sequence the store keeps, new with every write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PromptRevision(u64);

impl PromptRevision {
    /// The revision `number`. For adapters, and for a test.
    pub const fn new(number: u64) -> Self {
        PromptRevision(number)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PromptRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A prompt as a read returns it. For a prompt that is not the viewer's, `prompt().audience()` is
/// private and `own()` is false: the roles it was shared with are not the viewer's to know.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPrompt {
    prompt: PromptDefinition,
    revision: PromptRevision,
    updated_at: Timestamp,
    own: bool,
}

impl StoredPrompt {
    /// For adapters.
    pub fn new(
        prompt: PromptDefinition,
        revision: PromptRevision,
        updated_at: Timestamp,
        own: bool,
    ) -> Self {
        let prompt = if own {
            prompt
        } else {
            prompt.with_audience(PromptAudience::Private)
        };
        StoredPrompt {
            prompt,
            revision,
            updated_at,
            own,
        }
    }

    /// The prompt.
    pub fn prompt(&self) -> &PromptDefinition {
        &self.prompt
    }

    /// The revision a write based on it passes back.
    pub fn revision(&self) -> PromptRevision {
        self.revision
    }

    /// When it was last written (the time the writer passed).
    pub fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    /// Whether the viewer owns it.
    pub fn own(&self) -> bool {
        self.own
    }

    /// The prompt, without the rest.
    pub fn into_prompt(self) -> PromptDefinition {
        self.prompt
    }
}

/// One row of a listing: no body and no variables (a list of a hundred bodies would be megabytes).
#[derive(Debug, Clone, PartialEq)]
pub struct PromptSummary {
    /// The id.
    pub id: PromptId,
    /// The name.
    pub name: String,
    /// The title.
    pub title: String,
    /// The description.
    pub description: String,
    /// How many variables.
    pub variable_count: u8,
    /// Who it is shared with: the viewer's own prompts only.
    pub audience: Option<PromptAudience>,
    /// Whether the viewer owns it.
    pub own: bool,
    /// The revision.
    pub revision: PromptRevision,
    /// When it was last written.
    pub updated_at: Timestamp,
}

/// A page of a listing.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptPage {
    /// At most `limit`, ordered by id, bytewise.
    pub items: Vec<PromptSummary>,
    /// Whether more follow the last.
    pub more: bool,
    /// How many prompts the viewer owns, whatever the scope.
    pub held: u32,
}

/// Why a store refused or failed. None holds a prompt's words or an owner.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PromptStoreError {
    /// The owner already holds [`MAX_PROMPTS`]. Nothing was written.
    #[error("a person holds at most {limit} prompts")]
    TooMany {
        /// The limit.
        limit: usize,
    },
    /// [`insert_prompt`](PromptStore::insert_prompt) of an id somebody holds. Nothing was written.
    #[error("a prompt with that id exists")]
    Exists,
    /// The owner already has a prompt of that name. Nothing was written.
    #[error("the person already has a prompt of that name")]
    NameTaken,
    /// There is no such prompt for this owner: it was never saved, it was deleted, or it is
    /// another person's (the same to the caller).
    #[error("no such prompt")]
    NotFound,
    /// The prompt changed since the revision the write was based on. Nothing was written.
    #[error("the prompt changed since it was read")]
    Conflict,
    /// The store could not be reached or failed.
    #[error("the prompt store is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without words of a prompt.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// What the store holds is not a prompt. Alert.
    #[error("the prompt store holds corrupt data: {0}")]
    Corrupt(String),
    /// The store refused a request that no retry will change (a value the database cannot hold, a
    /// statement it rejects): a bug to fix, never a reason to wait. [`Unavailable`](Self::Unavailable)
    /// is the one a caller retries.
    #[error("the prompt store refused the request: {detail}")]
    Internal {
        /// What was attempted, without words of a prompt.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
}

impl PromptStoreError {
    /// The store failed or could not be reached.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        PromptStoreError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// The store refused a request that no retry will change.
    pub fn internal(detail: impl Into<String>) -> Self {
        PromptStoreError::Internal {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause of an `Unavailable` or `Internal` error; any other variant is
    /// returned unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            PromptStoreError::Unavailable { detail, .. } => PromptStoreError::Unavailable {
                detail,
                source: Some(source.into()),
            },
            PromptStoreError::Internal { detail, .. } => PromptStoreError::Internal {
                detail,
                source: Some(source.into()),
            },
            other @ (PromptStoreError::TooMany { .. }
            | PromptStoreError::Exists
            | PromptStoreError::NameTaken
            | PromptStoreError::NotFound
            | PromptStoreError::Conflict
            | PromptStoreError::Corrupt(_)) => other,
        }
    }
}

impl Classify for PromptStoreError {
    fn class(&self) -> ErrorClass {
        match self {
            PromptStoreError::TooMany { .. }
            | PromptStoreError::Exists
            | PromptStoreError::NameTaken => ErrorClass::Rejected,
            PromptStoreError::NotFound => ErrorClass::NotFound,
            PromptStoreError::Conflict => ErrorClass::Conflict,
            PromptStoreError::Unavailable { .. } => ErrorClass::Transient,
            PromptStoreError::Corrupt(_) => ErrorClass::Corrupt,
            PromptStoreError::Internal { .. } => ErrorClass::Internal,
        }
    }
}

/// The library of every person: their prompts, and the ones shared with them.
///
/// The methods carry `_prompt` because the supertraits of `ThreadStore` share one namespace on the
/// adapters, and a bare `get`, `list` or `delete` would be ambiguous at every call site that has
/// the traits in scope.
pub trait PromptStore: Send + Sync + 'static {
    /// Saves a new prompt for `owner`, written at `at`. The refusals are decided in this order:
    /// [`Exists`](PromptStoreError::Exists) (anybody holds the id),
    /// [`NameTaken`](PromptStoreError::NameTaken) (the owner has that name),
    /// [`TooMany`](PromptStoreError::TooMany).
    fn insert_prompt(
        &self,
        owner: &UserId,
        prompt: &PromptDefinition,
        at: Timestamp,
    ) -> impl Future<Output = Result<StoredPrompt, PromptStoreError>> + Send;

    /// The prompt `id` as `viewer` sees it: their own, or one shared with them; `None` for anything
    /// else, which is the same for a prompt that never existed.
    fn get_prompt(
        &self,
        viewer: &PromptViewer,
        id: &PromptId,
    ) -> impl Future<Output = Result<Option<StoredPrompt>, PromptStoreError>> + Send;

    /// A page of what `viewer` sees in `scope`, ordered by id (bytewise), after `after`
    /// (exclusive), at most `limit` (at most [`PROMPT_PAGE`]).
    fn list_prompts(
        &self,
        viewer: &PromptViewer,
        scope: PromptScope,
        after: Option<&PromptId>,
        limit: u32,
    ) -> impl Future<Output = Result<PromptPage, PromptStoreError>> + Send;

    /// Replaces the whole prompt `prompt.id()` of `owner` if it is still at `expected`, written at
    /// `at`: [`NotFound`](PromptStoreError::NotFound) when it is not the owner's or is gone,
    /// [`Conflict`](PromptStoreError::Conflict) when it changed, then
    /// [`NameTaken`](PromptStoreError::NameTaken) for a rename onto another of the owner's.
    fn replace_prompt(
        &self,
        owner: &UserId,
        prompt: &PromptDefinition,
        expected: PromptRevision,
        at: Timestamp,
    ) -> impl Future<Output = Result<StoredPrompt, PromptStoreError>> + Send;

    /// Deletes the owner's prompt `id`; with `expected`, only if it is still at that revision
    /// ([`Conflict`](PromptStoreError::Conflict) otherwise). [`NotFound`](PromptStoreError::NotFound)
    /// when it is not the owner's, which includes the second delete.
    fn delete_prompt(
        &self,
        owner: &UserId,
        id: &PromptId,
        expected: Option<PromptRevision>,
    ) -> impl Future<Output = Result<(), PromptStoreError>> + Send;
}
