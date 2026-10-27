//! What becomes of token usage reports that are not logged (ADR 0056, `usage/v1`): the counter
//! `usage_reports_dropped_total{reason}`, for this process.
//!
//! A report that breaks the contract is refused by the A2A adapter (`AgentUpdate::UsageRejected`)
//! and never reaches the core; a valid call report past the job's bound
//! ([`MAX_USAGE_CALLS_PER_JOB`](orch_core::MAX_USAGE_CALLS_PER_JOB)) is dropped by the core and
//! counted in the job ledger. Neither fails the task. A task's totals whose `sandbox` member broke
//! the contract are logged without it ([`UsageTotals::sandbox_rejected`](orch_core::UsageTotals)):
//! the tokens are kept, the member is counted as `sandbox_invalid`. The counter names no thread and
//! no agent; a debug line says which thread.

use std::sync::atomic::{AtomicU64, Ordering};

use orch_core::{ThreadId, UsageInvalid, UsageUpdate};
use orch_ports::Ports;

use super::App;

/// Why a usage report was not logged, for the counter's `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageDrop {
    /// It broke the contract (a missing member, a count out of range, a total that is not the sum).
    Invalid,
    /// It was a valid call report, and the job had logged as many as it may.
    JobLimit,
    /// It was a task's totals whose `sandbox` member broke the contract: the totals were logged,
    /// the member was not.
    SandboxInvalid,
}

impl UsageDrop {
    /// The label of the counter.
    pub const fn as_str(self) -> &'static str {
        match self {
            UsageDrop::Invalid => "invalid",
            UsageDrop::JobLimit => "job_limit",
            UsageDrop::SandboxInvalid => "sandbox_invalid",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct UsageCounters {
    invalid: AtomicU64,
    job_limit: AtomicU64,
    sandbox_invalid: AtomicU64,
}

/// The counter of usage reports this process did not log, by why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageStats {
    /// `usage_reports_dropped_total{reason}`.
    pub reports_dropped: [(UsageDrop, u64); 3],
}

impl<P: Ports> App<P> {
    /// The counter of usage reports this process did not log.
    pub fn usage_stats(&self) -> UsageStats {
        let c = &self.usage_counters;
        UsageStats {
            reports_dropped: [
                (UsageDrop::Invalid, c.invalid.load(Ordering::Relaxed)),
                (UsageDrop::JobLimit, c.job_limit.load(Ordering::Relaxed)),
                (
                    UsageDrop::SandboxInvalid,
                    c.sandbox_invalid.load(Ordering::Relaxed),
                ),
            ],
        }
    }

    /// A usage report of `thread` was not logged, for `why` (`detail` says more, for the log line).
    pub(crate) fn usage_dropped(&self, why: UsageDrop, thread: ThreadId, detail: &str) {
        let counter = match why {
            UsageDrop::Invalid => &self.usage_counters.invalid,
            UsageDrop::JobLimit => &self.usage_counters.job_limit,
            UsageDrop::SandboxInvalid => &self.usage_counters.sandbox_invalid,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(%thread, reason = why.as_str(), detail, "a usage report was not logged");
    }
}

/// Why the `sandbox` member of `update`, a task's totals, was left out by the adapter that read it;
/// `None` for anything else. The totals themselves are applied either way.
pub(crate) fn sandbox_rejection(update: &UsageUpdate) -> Option<UsageInvalid> {
    match update {
        UsageUpdate::Total(totals) => totals.sandbox_rejected,
        UsageUpdate::Call(_) => None,
    }
}
