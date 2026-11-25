//! The account-erase port: what deleting an account does to what a person owns (ADR 0098).
//!
//! There is no account row: a person is the key their credential gives, so an account is every
//! record whose owner is that key. One call erases them all and keeps only counts, for a set time.
//!
//! | Call | Does |
//! |---|---|
//! | [`erase`](AccountErase::erase) | Erases everything the person owns, in one decision, and says how many threads and files went and until when the counts stay. |
//!
//! # What an implementation owes
//!
//! - **Everything of the person goes, and nothing of anybody else.** Threads with all that hangs on
//!   them, the files (queued for the purge worker: [`EraseOutcome::files_pending`]), prompts, memory
//!   notes, push devices and the person's own tool servers. Forks other people made and prompts shared
//!   with the person are theirs and stay.
//! - **A job in progress refuses it**, [`EraseError::Busy`], and nothing is changed. The check and the
//!   erase are one decision: a thread that starts work in between makes the erase fail the same way,
//!   and nothing is half erased.
//! - **Only counts stay**: the rows of the usage ledger, under a pseudonym that is not the key and
//!   cannot be turned back into it, with the time they are kept until ([`EraseOutcome::kept_until`]).
//! - **A second call is [`EraseError::NotFound`]**: nothing is left of the account.
//! - **A `keep_days` of `0` is not a value** ([`EraseError::NoKeep`]) and nothing is changed.
//!
//! # Not in this port
//!
//! Revoking the person's tool servers at the credential broker comes before the erase and needs a
//! broker, the keep sweep runs later, and the route, the permission and the confirmation belong to the
//! API: slice E3 of ADR 0098 builds them over this port.
//!
//! # Open
//!
//! - The ADR gives no ceiling for `keep_days`; none is enforced here.
//! - The pseudonym's construction (a keyed hash with a deployment secret) is the adapter's; the
//!   testkit checks only that it is not the key, does not contain it, and differs between people.
//! - The late result of an erased thread (ADR 0043, decision 5) is dispatcher behaviour, not this
//!   port's: the testkit observes it through the fixture of the adapter (`testkit::account_erase`).

use std::future::Future;

use jiff::Timestamp;
use orch_core::{Classify, ErrorClass, UserId};

/// What an erase did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraseOutcome {
    /// Threads erased.
    pub threads_erased: u64,
    /// Files whose erase is queued: the purge worker follows.
    pub files_pending: u64,
    /// When the kept counts are deleted; `None` when the person had no usage rows.
    pub kept_until: Option<Timestamp>,
}

/// Why an erase did not happen. Nothing was changed in any of them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EraseError {
    /// Nothing is left of the account to delete (also the answer to a second call).
    #[error("no such account")]
    NotFound,
    /// A thread of the person, or an edit of one, is working or has a running ask.
    #[error("{running} of the threads of the account are working")]
    Busy {
        /// How many threads work.
        running: usize,
    },
    /// `keep_days` was `0`, or so large that the time it names is out of range.
    #[error("the keep period must be at least one day and a time that exists")]
    NoKeep,
    /// The store failed or could not be reached.
    #[error("the store is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without keys or content.
        detail: String,
    },
}

impl Classify for EraseError {
    fn class(&self) -> ErrorClass {
        match self {
            EraseError::NotFound => ErrorClass::NotFound,
            EraseError::Busy { .. } => ErrorClass::Rejected,
            EraseError::NoKeep => ErrorClass::Invalid,
            EraseError::Unavailable { .. } => ErrorClass::Transient,
        }
    }
}

/// Erases what a person owns and keeps only counts for a set time (ADR 0098).
///
/// See the module's docs for what an implementation owes. A call is one decision of the store.
pub trait AccountErase: Send + Sync + 'static {
    /// Erases the account of `account`, as of `now`, keeping the counts of its usage until `now`
    /// plus `keep_days` days.
    ///
    /// # Errors
    /// [`EraseError::NotFound`] when nothing is left of the account, [`EraseError::Busy`] while a
    /// thread works, [`EraseError::NoKeep`] for a `keep_days` of `0`, [`EraseError::Unavailable`] when
    /// the store fails.
    fn erase(
        &self,
        account: &UserId,
        now: Timestamp,
        keep_days: u32,
    ) -> impl Future<Output = Result<EraseOutcome, EraseError>> + Send;
}
