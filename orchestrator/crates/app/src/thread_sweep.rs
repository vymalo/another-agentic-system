//! The retention sweep of threads (`retention.threads.days`, default 90): a thread with no active
//! job whose last event is older than the retention is erased, through the same path as the
//! person's own delete (ADR 0043): the log, the files, the search index and the notifications go,
//! and a fork it was copied into is kept.
//!
//! A pass reads the idle threads oldest first ([`ThreadStore::idle_threads`](orch_ports::ThreadStore::idle_threads)),
//! a batch at a time after a cursor, so a thread the delete refuses (a running ask) never holds up
//! the ones behind it. It runs once when the process starts and then every hour, in the `worker`
//! and `all` roles; several replicas may run it, and an erasure that has been done is not done
//! again. One log line says how many threads a pass erased; it names no person and no thread.

use std::sync::Arc;
use std::time::Duration;

use orch_core::report;
use orch_ports::{Clock, Ports, ThreadStore as _};
use tokio_util::sync::CancellationToken;

use crate::{App, AppError};

/// Default of [`ThreadSweepConfig::interval`], seconds: an hour.
pub const DEFAULT_THREAD_SWEEP_INTERVAL_SECS: u64 = 3600;
/// Default of [`ThreadSweepConfig::batch`]: threads read and erased at a time.
pub const DEFAULT_THREAD_SWEEP_BATCH: u32 = 100;
/// The most batches one pass takes; a pass that has not caught up leaves the rest for the next.
const MAX_BATCHES_PER_PASS: u32 = 1000;

/// Tunables of the thread retention sweep.
#[derive(Debug, Clone)]
pub struct ThreadSweepConfig {
    /// How long the sweep waits between passes.
    pub interval: Duration,
    /// Threads one batch reads.
    pub batch: u32,
}

impl Default for ThreadSweepConfig {
    fn default() -> Self {
        ThreadSweepConfig {
            interval: Duration::from_secs(DEFAULT_THREAD_SWEEP_INTERVAL_SECS),
            batch: DEFAULT_THREAD_SWEEP_BATCH,
        }
    }
}

/// The thread retention sweep. See the module documentation.
pub struct ThreadSweepWorker<P: Ports> {
    app: Arc<App<P>>,
    cfg: ThreadSweepConfig,
}

impl<P: Ports> ThreadSweepWorker<P> {
    /// A sweep over `app`, for the retention `AppConfig::thread_retention_days` gives.
    pub fn new(app: Arc<App<P>>, cfg: ThreadSweepConfig) -> Arc<Self> {
        Arc::new(ThreadSweepWorker { app, cfg })
    }

    /// Runs until `shutdown` is cancelled: a pass now, then one every interval.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        while !shutdown.is_cancelled() {
            self.pass(&shutdown).await;
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(self.cfg.interval) => {}
            }
        }
    }

    /// One pass to the end; returns how many threads it erased (edits and children included).
    /// [`run`](Self::run) is this in a loop; tests call it directly.
    pub async fn sweep(&self) -> u64 {
        self.pass(&CancellationToken::new()).await
    }

    async fn pass(&self, shutdown: &CancellationToken) -> u64 {
        let days = i64::from(self.app.thread_retention_days());
        let now = self.app.ports().clock().now();
        let Ok(before) = now.checked_sub(jiff::SignedDuration::from_hours(24 * days)) else {
            return 0;
        };
        let batch = self.cfg.batch.max(1);
        let store = self.app.ports().store();
        let mut erased = 0_u64;
        let mut after = None;
        for _ in 0..MAX_BATCHES_PER_PASS {
            if shutdown.is_cancelled() {
                break;
            }
            let idle = match store.idle_threads(before, after, batch).await {
                Ok(idle) => idle,
                Err(e) => {
                    tracing::warn!(error = %report(&e), "the thread retention sweep failed");
                    break;
                }
            };
            let Some(last) = idle.last() else { break };
            after = Some((last.updated_at, last.id));
            let full = idle.len() >= batch as usize;
            for thread in idle {
                match self.app.erase_idle(&thread.owner, thread.id).await {
                    Ok(n) => erased += n as u64,
                    // it works, was deleted meanwhile, or kept changing: not this pass's
                    Err(AppError::ThreadActive(_) | AppError::NotFound | AppError::Contended) => {}
                    Err(e) => {
                        tracing::warn!(error = %report(&e), "a thread could not be erased by the retention sweep");
                    }
                }
            }
            if !full {
                break;
            }
        }
        if erased > 0 {
            tracing::info!(
                retention_days = days,
                threads = erased,
                "idle threads were erased by the retention sweep"
            );
        }
        erased
    }
}
