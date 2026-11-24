//! A person's team presets and the port that keeps them (ADR 0074, ADR 0009).
//!
//! A preset is a saved team: the lead, two to eight members with their agents and roles, a charter,
//! the cap of hand-offs, when a hand-off waits for the person, the checkpoint rules, the budget of a
//! push and whether the older entries are summarised. The rules of what a preset is are
//! [`orch_core::team`]; what is kept here is the definitions, per person, and their revisions. No
//! event of the log records one: a thread that a preset started keeps a copy in `team_formed`,
//! never a reference, so deleting a preset changes no conversation and deleting a thread changes no
//! preset.
//!
//! # The port is a supertrait of [`ThreadStore`](crate::ThreadStore)
//!
//! **Every `ThreadStore` is a [`TeamPresetStore`]: an adapter for another database implements it
//! beside its threads.** The presets live in the thread store's database and pool, and a port of
//! its own would need a new associated type and a new field in every `PortSet` literal. This is a
//! breaking change for every implementer of `ThreadStore` (four in this workspace: `MemoryStore`,
//! `PgStore`, `SqliteStore` and the test double of `orch-app`'s inbox tests).
//!
//! # Who sees what, decided by the store
//!
//! Presets are each person's own in this version (ADR 0074 decision 13; sharing by role comes
//! later, on ADR 0070's model). Every method therefore takes the person, and **the owner predicate
//! is in the store's own query, never in a handler**: no type of the port carries an owner, and a
//! preset that is not the person's comes back as `None` or [`NotFound`](TeamPresetStoreError::NotFound),
//! the same answer as for an id that never existed. Nothing tells a caller that somebody else's
//! preset holds that id.
//!
//! # Writes
//!
//! Every write that changes an existing preset is a **compare and swap** on a
//! [`TeamPresetRevision`] the store keeps and a read returns: a stale one is
//! [`Conflict`](TeamPresetStoreError::Conflict), an absent one is
//! [`NotFound`](TeamPresetStoreError::NotFound) (a deleted preset stays deleted), and only
//! [`insert_team_preset`](TeamPresetStore::insert_team_preset) creates. A revision is a number of
//! one sequence, **never given twice**, a delete included. At most [`MAX_TEAM_PRESETS`] per owner,
//! **enforced by the store**; the refusals of an insert are decided in the order
//! [`Exists`](TeamPresetStoreError::Exists), [`NameTaken`](TeamPresetStoreError::NameTaken),
//! [`TooMany`](TeamPresetStoreError::TooMany), so every adapter refuses the same way.
//!
//! # A listing's limit is positive
//!
//! A store returns at least one preset to a listing and at most [`TEAM_PRESET_PAGE`]: a `limit` of
//! zero is one, and no store needs to refuse it. The bound is the store's, whatever the caller asks,
//! as a page of prompts is.

use std::fmt;
use std::future::Future;

use orch_core::{BoxError, Classify, ErrorClass, Timestamp, UserId};
use orch_core::{MemberName, TeamPreset, TeamPresetId};

pub use orch_core::{MAX_TEAM_PRESETS, TEAM_PRESET_PAGE};

/// The revision of a team preset: a number of one sequence the store keeps, new with every write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TeamPresetRevision(u64);

impl TeamPresetRevision {
    /// The revision `number`. For adapters, and for a test.
    pub const fn new(number: u64) -> Self {
        TeamPresetRevision(number)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TeamPresetRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A team preset as a read returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredTeamPreset {
    preset: TeamPreset,
    revision: TeamPresetRevision,
    updated_at: Timestamp,
}

impl StoredTeamPreset {
    /// For adapters.
    pub fn new(preset: TeamPreset, revision: TeamPresetRevision, updated_at: Timestamp) -> Self {
        StoredTeamPreset {
            preset,
            revision,
            updated_at,
        }
    }

    /// The preset.
    pub fn preset(&self) -> &TeamPreset {
        &self.preset
    }

    /// The revision a write based on it passes back.
    pub fn revision(&self) -> TeamPresetRevision {
        self.revision
    }

    /// When it was last written (the time the writer passed).
    pub fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    /// The preset, without the rest.
    pub fn into_preset(self) -> TeamPreset {
        self.preset
    }
}

/// One row of a listing: enough to draw a picker, without the charter or the members' roles (fifty
/// charters of 4 KiB would be a quarter of a megabyte for a list).
#[derive(Debug, Clone, PartialEq)]
pub struct TeamPresetSummary {
    /// The id.
    pub id: TeamPresetId,
    /// The name.
    pub name: String,
    /// The title.
    pub title: String,
    /// The member an unaddressed message goes to.
    pub lead: MemberName,
    /// How many members the team has.
    pub member_count: u8,
    /// The revision.
    pub revision: TeamPresetRevision,
    /// When it was last written.
    pub updated_at: Timestamp,
}

/// A page of a person's presets.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamPresetPage {
    /// Between one and [`TEAM_PRESET_PAGE`], ordered by id, bytewise.
    pub items: Vec<TeamPresetSummary>,
    /// Whether more follow the last.
    pub more: bool,
    /// How many presets the person holds, whatever the page.
    pub held: u32,
}

/// Why a store refused or failed. None holds a preset's words or an owner.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TeamPresetStoreError {
    /// The owner already holds [`MAX_TEAM_PRESETS`]. Nothing was written.
    #[error("a person holds at most {limit} team presets")]
    TooMany {
        /// The limit.
        limit: usize,
    },
    /// [`insert_team_preset`](TeamPresetStore::insert_team_preset) of an id somebody holds. Nothing
    /// was written.
    #[error("a team preset with that id exists")]
    Exists,
    /// The owner already has a preset of that name. Nothing was written.
    #[error("the person already has a team preset of that name")]
    NameTaken,
    /// There is no such preset for this owner: it was never saved, it was deleted, or it is
    /// another person's (the same to the caller).
    #[error("no such team preset")]
    NotFound,
    /// The preset changed since the revision the write was based on. Nothing was written.
    #[error("the team preset changed since it was read")]
    Conflict,
    /// The store could not be reached or failed.
    #[error("the team preset store is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without words of a preset.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// What the store holds is not a team preset. Alert.
    #[error("the team preset store holds corrupt data: {0}")]
    Corrupt(String),
    /// The store refused a request that no retry will change (a value the database cannot hold, a
    /// statement it rejects): a bug to fix, never a reason to wait.
    /// [`Unavailable`](Self::Unavailable) is the one a caller retries.
    #[error("the team preset store refused the request: {detail}")]
    Internal {
        /// What was attempted, without words of a preset.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
}

impl TeamPresetStoreError {
    /// The store failed or could not be reached.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        TeamPresetStoreError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// The store refused a request that no retry will change.
    pub fn internal(detail: impl Into<String>) -> Self {
        TeamPresetStoreError::Internal {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause of an `Unavailable` or `Internal` error; any other variant is
    /// returned unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            TeamPresetStoreError::Unavailable { detail, .. } => TeamPresetStoreError::Unavailable {
                detail,
                source: Some(source.into()),
            },
            TeamPresetStoreError::Internal { detail, .. } => TeamPresetStoreError::Internal {
                detail,
                source: Some(source.into()),
            },
            other @ (TeamPresetStoreError::TooMany { .. }
            | TeamPresetStoreError::Exists
            | TeamPresetStoreError::NameTaken
            | TeamPresetStoreError::NotFound
            | TeamPresetStoreError::Conflict
            | TeamPresetStoreError::Corrupt(_)) => other,
        }
    }
}

impl Classify for TeamPresetStoreError {
    fn class(&self) -> ErrorClass {
        match self {
            TeamPresetStoreError::TooMany { .. }
            | TeamPresetStoreError::Exists
            | TeamPresetStoreError::NameTaken => ErrorClass::Rejected,
            TeamPresetStoreError::NotFound => ErrorClass::NotFound,
            TeamPresetStoreError::Conflict => ErrorClass::Conflict,
            TeamPresetStoreError::Unavailable { .. } => ErrorClass::Transient,
            TeamPresetStoreError::Corrupt(_) => ErrorClass::Corrupt,
            TeamPresetStoreError::Internal { .. } => ErrorClass::Internal,
        }
    }
}

/// A person's team presets.
///
/// The methods carry `_team_preset` because the supertraits of `ThreadStore` share one namespace on
/// the adapters, and a bare `get`, `list` or `delete` would be ambiguous at every call site that has
/// the traits in scope.
pub trait TeamPresetStore: Send + Sync + 'static {
    /// Saves a new preset for `owner`, written at `at`. The refusals are decided in this order:
    /// [`Exists`](TeamPresetStoreError::Exists) (anybody holds the id),
    /// [`NameTaken`](TeamPresetStoreError::NameTaken) (the owner has that name),
    /// [`TooMany`](TeamPresetStoreError::TooMany).
    fn insert_team_preset(
        &self,
        owner: &UserId,
        preset: &TeamPreset,
        at: Timestamp,
    ) -> impl Future<Output = Result<StoredTeamPreset, TeamPresetStoreError>> + Send;

    /// The owner's preset `id`; `None` when it is not theirs, which is the same for one that never
    /// existed.
    fn get_team_preset(
        &self,
        owner: &UserId,
        id: &TeamPresetId,
    ) -> impl Future<Output = Result<Option<StoredTeamPreset>, TeamPresetStoreError>> + Send;

    /// A page of the owner's presets, ordered by id (bytewise), after `after` (exclusive), at least
    /// one and at most [`TEAM_PRESET_PAGE`]: a `limit` of zero is one.
    fn list_team_presets(
        &self,
        owner: &UserId,
        after: Option<&TeamPresetId>,
        limit: u32,
    ) -> impl Future<Output = Result<TeamPresetPage, TeamPresetStoreError>> + Send;

    /// Replaces the whole preset `preset.id()` of `owner` if it is still at `expected`, written at
    /// `at`: [`NotFound`](TeamPresetStoreError::NotFound) when it is not the owner's or is gone,
    /// [`Conflict`](TeamPresetStoreError::Conflict) when it changed, then
    /// [`NameTaken`](TeamPresetStoreError::NameTaken) for a rename onto another of the owner's.
    fn replace_team_preset(
        &self,
        owner: &UserId,
        preset: &TeamPreset,
        expected: TeamPresetRevision,
        at: Timestamp,
    ) -> impl Future<Output = Result<StoredTeamPreset, TeamPresetStoreError>> + Send;

    /// Deletes the owner's preset `id`; with `expected`, only if it is still at that revision
    /// ([`Conflict`](TeamPresetStoreError::Conflict) otherwise).
    /// [`NotFound`](TeamPresetStoreError::NotFound) when it is not the owner's, which includes the
    /// second delete.
    fn delete_team_preset(
        &self,
        owner: &UserId,
        id: &TeamPresetId,
        expected: Option<TeamPresetRevision>,
    ) -> impl Future<Output = Result<(), TeamPresetStoreError>> + Send;
}
