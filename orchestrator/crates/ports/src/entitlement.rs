//! The `EntitlementSource` port: whether a person's plan covers them right now.
//!
//! A person's own agents (their coder and their workspace, [`AgentProvisioner`](crate::AgentProvisioner))
//! run for as long as their plan does. The plan is the platform's fact and not the orchestrator's, so
//! this port is the only place the orchestrator asks: the answer decides whether those agents run, and
//! a lapse is what the orchestrator answers with `spec.suspend`.
//!
//! | Answer | What it means | What the caller does |
//! |---|---|---|
//! | [`Entitlement::Active`] | the plan covers them now | leaves their agents alone (and may resume the ones a lapse suspended) |
//! | [`Entitlement::Lapsed`] with its `since` | the plan ended at `since` | suspends their agents |
//! | [`Entitlement::Unknown`] | the source has no record of them at all | suspends their agents too, and says so in the log: a person the plan source never heard of is an identity mapping to fix |
//! | [`EntitlementError::Unavailable`] | the source could not answer | **does nothing**: an outage is not a lapse, and suspending everyone because one read failed would be an outage that suspends the estate |
//!
//! # A read, not a watch
//!
//! There is deliberately no stream of changes here. [`Wakeup`](crate::Wakeup) carries hints about the
//! orchestrator's own state (a thread, the outbox, the inbox) and never about a fact of another
//! system; a subscription to the plan source would need a protocol, reconnect and resume story of its
//! own, and would still need a polling path for a change announced while a consumer was down, because
//! a notification is never the truth. So the caller polls this read, on the schedule it already keeps
//! for its other sweeps.
//!
//! # How a lapse is noticed
//!
//! Nothing announces it: a **sweep worker** asks, the same shape as the usage retention sweep
//! (`UsageSweepWorker` in `orchestrator/crates/app/src/usage_sweep.rs`: a pass when the process starts,
//! then one every interval, running wherever the dispatcher runs and safe on several replicas because
//! it only reads and the suspend it then asks for is idempotent). The worker walks the people whose
//! agents exist, calls [`entitled`](EntitlementSource::entitled) for each, and hands a non-active
//! answer to [`AgentProvisioner::suspend`](crate::AgentProvisioner::suspend) for the kinds that person
//! has. The message path may also ask once before
//! [`ensure_ready`](crate::AgentProvisioner::ensure_ready) so a renewal takes effect on the next
//! message instead of at the next pass; both callers share this one read.
//!
//! # What an implementation owes
//!
//! - **The answer is that person's alone.** A source never answers from another person's record, and
//!   never takes one person's state as the default for the next.
//! - **`Active` is never served from a cache.** A plan that lapsed reads
//!   [`Lapsed`](Entitlement::Lapsed) on the first read after the lapse; the sweep's interval bounds
//!   how late that read is, not a cached answer's lifetime.
//! - **A person with no record is [`Unknown`](Entitlement::Unknown)**, never
//!   [`Active`](Entitlement::Active): nothing of a person the plan source does not know may run.
//! - **A failing source is an error, not a verdict.** A read that cannot be made is
//!   [`EntitlementError::Unavailable`] (class `Transient`), so the caller keeps the state it has
//!   instead of acting on a guess.
//! - **Nothing in an error, a log line or a `Debug` names a person or another person's
//!   entitlement.** The errors carry no identity at all, and [`Entitlement`] carries no person.

use std::future::Future;

use jiff::Timestamp;
use orch_core::{BoxError, Classify, ErrorClass, UserId};

/// Whether a person's plan covers them right now.
///
/// Closed on purpose: a new answer must fail to compile at every caller, so nobody reads a new state
/// as "probably active".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entitlement {
    /// The plan covers the person now.
    Active,
    /// The plan ended at `since`; nothing of theirs runs until it is renewed.
    Lapsed {
        /// The moment the plan lapsed, as the source knows it.
        since: Timestamp,
    },
    /// The source holds no record of the person. Not [`Active`](Self::Active): the caller suspends.
    Unknown,
}

impl Entitlement {
    /// Whether the person may run their agents now: true for [`Active`](Self::Active) alone, so
    /// [`Lapsed`](Self::Lapsed) and [`Unknown`](Self::Unknown) both answer no (fail closed).
    pub fn is_active(self) -> bool {
        matches!(self, Entitlement::Active)
    }

    /// The moment it lapsed, when it did.
    pub fn lapsed_since(self) -> Option<Timestamp> {
        match self {
            Entitlement::Lapsed { since } => Some(since),
            Entitlement::Active | Entitlement::Unknown => None,
        }
    }
}

/// Why the source could not say whether a person is entitled. None holds a person or an entitlement.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EntitlementError {
    /// The source could not be reached or failed. The caller knows nothing new and must not act.
    #[error("the entitlement source is unavailable")]
    Unavailable {
        /// The driver's error.
        #[source]
        source: BoxError,
    },
    /// The source refused the orchestrator's credentials or its permission.
    #[error("the entitlement source refused the orchestrator")]
    Refused,
    /// The source answered something that cannot be read as an entitlement.
    #[error("the entitlement source answered something the port cannot read")]
    Invalid,
}

impl EntitlementError {
    /// The source is down or answered nothing.
    pub fn unavailable(source: impl Into<BoxError>) -> Self {
        EntitlementError::Unavailable {
            source: source.into(),
        }
    }
}

impl Classify for EntitlementError {
    fn class(&self) -> ErrorClass {
        match self {
            EntitlementError::Unavailable { .. } => ErrorClass::Transient,
            EntitlementError::Refused => ErrorClass::Unauthenticated,
            EntitlementError::Invalid => ErrorClass::Invalid,
        }
    }
}

/// Whether a person's plan covers them right now. See the module's docs for what an implementation
/// owes.
pub trait EntitlementSource: Send + Sync + 'static {
    /// `person`'s entitlement now: [`Entitlement::Active`], [`Entitlement::Lapsed`] with the moment
    /// it ended, or [`Entitlement::Unknown`] for a person the source holds no record of. Never a
    /// cached `Active`.
    ///
    /// # Errors
    /// [`EntitlementError::Unavailable`] (class `Transient`) when the source fails: the caller then
    /// knows nothing new and must not treat the person as lapsed. [`EntitlementError::Refused`] and
    /// [`EntitlementError::Invalid`] are the orchestrator's own problem and no read will fix them.
    fn entitled(
        &self,
        person: &UserId,
    ) -> impl Future<Output = Result<Entitlement, EntitlementError>> + Send;
}
