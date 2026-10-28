//! The retention sweep of the usage ledger (ADR 0065, decision 8): removes the months older than
//! `usage.retentionMonths` so that the ledger does not grow for ever.
//!
//! A pass takes the first month to keep (the current UTC month is one of the months kept, so 24
//! keeps this month and the 23 before it) and removes the tasks of every month before it, a batch
//! at a time ([`UsageLedger::sweep`](orch_ports::UsageLedger::sweep): the oldest first, no long
//! lock), until a batch comes back short. It runs once when the process starts and then every hour.
//! Several replicas may run it: a batch skips the rows another one holds and removes the others,
//! and a removal that has been done is not done again.
//!
//! One line of the log says what a pass removed, when it removed anything: the first month kept,
//! how many months the retention is, and how many tasks went. It names no person and no content.
//!
//! It runs wherever the dispatcher runs (the `worker` and `all` roles).

use std::sync::Arc;
use std::time::Duration;

use orch_core::{UsageMonth, report};
use orch_ports::{Clock, Ports, UsageLedger as _};
use tokio_util::sync::CancellationToken;

use crate::App;

/// Default of [`UsageSweepConfig::interval`], seconds: an hour.
pub const DEFAULT_USAGE_SWEEP_INTERVAL_SECS: u64 = 3600;
/// Default of [`UsageSweepConfig::batch`]: tasks removed by one statement, so one lock is short.
pub const DEFAULT_USAGE_SWEEP_BATCH: u32 = 1000;
/// Default of [`UsageSweepConfig::pause`], milliseconds, between two full batches of a backlog.
pub const DEFAULT_USAGE_SWEEP_PAUSE_MS: u64 = 100;
/// The most batches one pass takes: a pass that has not caught up by then leaves the rest for the
/// next hour instead of running for ever (a million tasks at the default batch).
const MAX_BATCHES_PER_PASS: u32 = 1000;

/// Tunables of the usage retention sweep.
#[derive(Debug, Clone)]
pub struct UsageSweepConfig {
    /// How long the sweep waits between passes.
    pub interval: Duration,
    /// Tasks one removal takes at most. A pass removes batches until one comes back short.
    pub batch: u32,
    /// How long a pass waits between two full batches, so a backlog is not removed in one burst.
    pub pause: Duration,
}

impl Default for UsageSweepConfig {
    fn default() -> Self {
        UsageSweepConfig {
            interval: Duration::from_secs(DEFAULT_USAGE_SWEEP_INTERVAL_SECS),
            batch: DEFAULT_USAGE_SWEEP_BATCH,
            pause: Duration::from_millis(DEFAULT_USAGE_SWEEP_PAUSE_MS),
        }
    }
}

/// The usage retention sweep. See the module documentation.
pub struct UsageSweepWorker<P: Ports> {
    app: Arc<App<P>>,
    cfg: UsageSweepConfig,
}

impl<P: Ports> UsageSweepWorker<P> {
    /// A sweep over `app`, for the retention the app's `usage` settings give.
    pub fn new(app: Arc<App<P>>, cfg: UsageSweepConfig) -> Arc<Self> {
        Arc::new(UsageSweepWorker { app, cfg })
    }

    /// Runs until `shutdown` is cancelled: a pass now, then one every
    /// [`interval`](UsageSweepConfig::interval).
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        while !shutdown.is_cancelled() {
            self.pass(&shutdown).await;
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(self.cfg.interval) => {}
            }
        }
    }

    /// One pass to the end: removes every task older than the retention (at most
    /// [`MAX_BATCHES_PER_PASS`] batches) and returns how many it removed. [`run`](Self::run) is this
    /// in a loop; tests call it to drive the sweep with no timing involved.
    pub async fn sweep(&self) -> u64 {
        self.pass(&CancellationToken::new()).await
    }

    async fn pass(&self, shutdown: &CancellationToken) -> u64 {
        let retention = self.app.usage_settings().retention_months;
        let current = UsageMonth::of(self.app.ports().clock().now());
        let before = current.first_kept(retention);
        let batch = self.cfg.batch.max(1);
        let store = self.app.ports().store();
        let mut removed = 0_u64;
        for _ in 0..MAX_BATCHES_PER_PASS {
            match store.sweep(before, batch).await {
                Ok(n) => {
                    removed += n;
                    if n < u64::from(batch) {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %report(&e), "the usage retention sweep failed");
                    break;
                }
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(self.cfg.pause) => {}
            }
        }
        if removed > 0 {
            self.app.usage_swept(removed);
            tracing::info!(
                first_month_kept = %before,
                retention_months = retention,
                tasks = removed,
                "the usage ledger was swept"
            );
        }
        removed
    }
}
