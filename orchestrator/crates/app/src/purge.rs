//! The purge sweep: finishes the erasure of the files of deleted threads that the inline purge of
//! [`App::delete_thread`] did not (ADR 0043, decision 2).
//!
//! A delete writes one `thread_purges` row per thread in the transaction that deletes the log,
//! then erases the files and removes the row. When the artifact store was down, or the process died
//! between the two, the row stays; this worker claims it under a lease
//! ([`ThreadStore::claim_purges`], `FOR UPDATE SKIP LOCKED` on Postgres, so replicas never share a
//! row), erases the files ([`ArtifactStore::delete_prefix`](orch_ports::ArtifactStore), idempotent,
//! so a claim that lapsed and is taken over does no harm) and finishes the row. A purge is a
//! promise: it is tried again and again, the lease lapsing between tries, and never given up on.
//!
//! It runs wherever the dispatcher runs (the `worker` and `all` roles).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use orch_core::report;
use orch_ports::{Clock, Ports, ThreadStore, Topic, Wakeup};
use tokio_util::sync::CancellationToken;

use crate::App;

/// Tunables of the purge sweep.
#[derive(Debug, Clone)]
pub struct PurgeConfig {
    /// How long a claim lasts. A worker that dies holds its rows this long; a claim that lapses
    /// while the files are still being erased is taken over, which does no harm.
    pub lease: Duration,
    /// How long the sweep waits between passes when it had nothing to do.
    pub poll_interval: Duration,
    /// Threads claimed at once.
    pub batch: u32,
}

/// Default of [`PurgeConfig::lease`], seconds.
pub const DEFAULT_PURGE_LEASE_SECS: u64 = 120;
/// Default of [`PurgeConfig::poll_interval`], seconds.
pub const DEFAULT_PURGE_POLL_SECS: u64 = 30;

impl Default for PurgeConfig {
    fn default() -> Self {
        PurgeConfig {
            lease: Duration::from_secs(DEFAULT_PURGE_LEASE_SECS),
            poll_interval: Duration::from_secs(DEFAULT_PURGE_POLL_SECS),
            batch: 16,
        }
    }
}

/// The purge sweep. See the module documentation.
pub struct PurgeWorker<P: Ports> {
    app: Arc<App<P>>,
    cfg: PurgeConfig,
    owner: String,
}

impl<P: Ports> PurgeWorker<P> {
    /// `owner` identifies this process in leases (unique per replica).
    pub fn new(app: Arc<App<P>>, cfg: PurgeConfig, owner: impl Into<String>) -> Arc<Self> {
        Arc::new(PurgeWorker {
            app,
            cfg,
            owner: owner.into(),
        })
    }

    /// Runs until `shutdown` is cancelled. Its leases are not released: a purge that was claimed
    /// and not finished is claimed again when the lease lapses.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        // A restart is a reason to look at once: the rows an earlier process left are due.
        let mut wake = self.app.ports().wakeup().subscribe();
        let mut wake_open = true;
        while !shutdown.is_cancelled() {
            if self.tick().await >= self.cfg.batch as usize {
                continue;
            }
            let tick = tokio::time::sleep(self.cfg.poll_interval);
            tokio::pin!(tick);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    // a lost notification: look now, it costs one query
                    topic = wake.next(), if wake_open => match topic {
                        Some(Topic::Resync) => break,
                        Some(_) => {}
                        None => wake_open = false,
                    },
                    () = &mut tick => break,
                }
            }
        }
    }

    /// One pass: claims the purges that are due (at most one batch) and finishes them. Returns how
    /// many it claimed. [`run`](Self::run) is this in a loop; tests call it to drive the sweep one
    /// step at a time, with no timing involved.
    pub async fn tick(&self) -> usize {
        let now = self.app.ports().clock().now();
        let claimed = match self
            .app
            .ports()
            .store()
            .claim_purges(&self.owner, self.cfg.batch.max(1), self.cfg.lease, now)
            .await
        {
            Ok(threads) => threads,
            Err(e) => {
                tracing::warn!(error = %report(&e), "claiming purges failed");
                return 0;
            }
        };
        let count = claimed.len();
        for thread in claimed {
            self.app.purge_files(thread).await;
        }
        count
    }
}
