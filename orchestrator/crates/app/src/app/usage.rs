//! What becomes of token usage reports that are not logged (ADR 0056, `usage/v1`): the counter
//! `usage_reports_dropped_total{reason}`, for this process.
//!
//! A report that breaks the contract is refused by the A2A adapter (`AgentUpdate::UsageRejected`)
//! and never reaches the core; a valid call report past the job's bound
//! ([`MAX_USAGE_CALLS_PER_JOB`](orch_core::MAX_USAGE_CALLS_PER_JOB)) is dropped by the core and
//! counted in the job ledger. Neither fails the task. The counter names no thread and no agent;
//! a debug line says which thread.

use std::sync::atomic::{AtomicU64, Ordering};

use orch_core::ThreadId;
use orch_ports::Ports;

use super::App;

/// Why a usage report was not logged, for the counter's `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageDrop {
    /// It broke the contract (a missing member, a count out of range, a total that is not the sum).
    Invalid,
    /// It was a valid call report, and the job had logged as many as it may.
    JobLimit,
}

impl UsageDrop {
    /// The label of the counter.
    pub const fn as_str(self) -> &'static str {
        match self {
            UsageDrop::Invalid => "invalid",
            UsageDrop::JobLimit => "job_limit",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct UsageCounters {
    invalid: AtomicU64,
    job_limit: AtomicU64,
}

/// The counter of usage reports this process did not log, by why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageStats {
    /// `usage_reports_dropped_total{reason}`.
    pub reports_dropped: [(UsageDrop, u64); 2],
}

impl<P: Ports> App<P> {
    /// The counter of usage reports this process did not log.
    pub fn usage_stats(&self) -> UsageStats {
        let c = &self.usage_counters;
        UsageStats {
            reports_dropped: [
                (UsageDrop::Invalid, c.invalid.load(Ordering::Relaxed)),
                (UsageDrop::JobLimit, c.job_limit.load(Ordering::Relaxed)),
            ],
        }
    }

    /// A usage report of `thread` was not logged, for `why` (`detail` says more, for the log line).
    pub(crate) fn usage_dropped(&self, why: UsageDrop, thread: ThreadId, detail: &str) {
        let counter = match why {
            UsageDrop::Invalid => &self.usage_counters.invalid,
            UsageDrop::JobLimit => &self.usage_counters.job_limit,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(%thread, reason = why.as_str(), detail, "a usage report was not logged");
    }
}
