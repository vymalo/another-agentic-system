//! What becomes of token usage reports that are not logged (ADR 0056, `usage/v1`): the counter
//! `usage_reports_dropped_total{reason}`, for this process. And the usage ledger of the people
//! (ADR 0065): the settings of its retention and prices, the priced month `GET /api/me/usage`
//! answers, and the counter of the tasks the retention sweep removed.
//!
//! A report that breaks the contract is refused by the A2A adapter (`AgentUpdate::UsageRejected`)
//! and never reaches the core; a valid call report past the job's bound
//! ([`MAX_USAGE_CALLS_PER_JOB`](orch_core::MAX_USAGE_CALLS_PER_JOB)) is dropped by the core and
//! counted in the job ledger. Neither fails the task. A task's totals whose `sandbox` member broke
//! the contract are logged without it ([`UsageTotals::sandbox_rejected`](orch_core::UsageTotals)):
//! the tokens are kept, the member is counted as `sandbox_invalid`. The counter names no thread and
//! no agent; a debug line says which thread.

use std::sync::atomic::{AtomicU64, Ordering};

use orch_core::{
    PriceTable, PricedMonth, ThreadId, UsageInvalid, UsageMonth, UsageUpdate, UserId, price_month,
};
use orch_ports::{Clock, Ports, UsageLedger};

use super::App;
use crate::AppError;

/// The default of [`UsageSettings::retention_months`]: two years.
pub const DEFAULT_USAGE_RETENTION_MONTHS: u32 = 24;

/// The settings of the usage ledger (ADR 0065): `usage` of the configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSettings {
    /// How many UTC calendar months the ledger keeps, **the current month included** (24 keeps this
    /// month and the 23 before it): the retention sweep removes the older ones. 1 to 1200 in the
    /// configuration file.
    pub retention_months: u32,
    /// The prices the answer of a month is made with. `None`: nothing is priced, and every line of
    /// the answer says so.
    pub prices: Option<PriceTable>,
}

impl Default for UsageSettings {
    fn default() -> Self {
        UsageSettings {
            retention_months: DEFAULT_USAGE_RETENTION_MONTHS,
            prices: None,
        }
    }
}

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
    swept: AtomicU64,
}

/// The counter of usage reports this process did not log, by why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageStats {
    /// `usage_reports_dropped_total{reason}`.
    pub reports_dropped: [(UsageDrop, u64); 3],
    /// `usage_tasks_swept_total`: the tasks of the usage ledger this process's retention sweep
    /// removed (ADR 0065).
    pub tasks_swept: u64,
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
            tasks_swept: c.swept.load(Ordering::Relaxed),
        }
    }

    /// The settings of the usage ledger: how long it keeps a month, and the prices.
    pub fn usage_settings(&self) -> &UsageSettings {
        &self.cfg.usage
    }

    /// What `owner` used in `month` (the current UTC month when `None`), priced with the configured
    /// table (ADR 0065, `GET /api/me/usage`). A person used nothing in a month that is not in the
    /// ledger: the answer is then empty and complete. Nothing else of the ledger is reachable from
    /// here: it is `owner`'s own month, and the caller passes the authenticated person.
    ///
    /// # Errors
    ///
    /// [`AppError::Store`] when the ledger cannot be read.
    pub async fn usage_month(
        &self,
        owner: &UserId,
        month: Option<UsageMonth>,
    ) -> Result<PricedMonth, AppError> {
        let month = month.unwrap_or_else(|| UsageMonth::of(self.ports.clock().now()));
        let totals = self.ports.store().month(owner, month).await?;
        Ok(price_month(self.cfg.usage.prices.as_ref(), &totals))
    }

    /// The retention sweep removed `tasks` tasks.
    pub(crate) fn usage_swept(&self, tasks: u64) {
        self.usage_counters
            .swept
            .fetch_add(tasks, Ordering::Relaxed);
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
