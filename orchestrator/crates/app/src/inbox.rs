//! The inbox worker: claims the unsolicited input that waits in the inbox (a timer that came
//! due, a CI report a webhook stored) and applies it to its thread through
//! [`App::apply_from_inbox`] (ADR 0016).
//!
//! It runs wherever the dispatcher runs (the `worker` and `all` roles); a control plane only
//! writes rows. Crash safety rests on the same facts as the dispatcher's:
//!
//! - a claim is a lease, and a dead worker's row becomes claimable again when it expires;
//! - the row is marked applied in the very commit that applies it, fenced by the lease, so a
//!   worker that was paused past its lease cannot apply a row twice or over another worker;
//! - the event the input produces carries the key `inbox:<row id>`, so even a row applied twice
//!   would add its events once.
//!
//! The states of a row, and the flow from a webhook to a commit, are drawn in
//! [`docs/orchestrator.md`](../../../../docs/orchestrator.md#inbox-timers-and-watches).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use jiff::{SignedDuration, Timestamp};
use orch_core::{Classify, Input, ThreadId, report};
use orch_ports::{
    Clock, InboxFinal, InboxItem, InboxLease, InboxPayload, Parking, Ports, StoreError,
    ThreadStore, Topic, Wakeup,
};
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::{App, AppError, ApplyOutcome, LateSource};

/// Tunables of the inbox worker.
#[derive(Debug, Clone)]
pub struct InboxConfig {
    /// How long a claim lasts. One row is one short commit, so this only has to outlive a slow
    /// database call; a worker that dies holds its rows this long.
    pub lease: Duration,
    /// Safety poll when no wakeup arrives. Timers coming due are not announced, so this is also
    /// how late a timer can fire.
    pub poll_interval: Duration,
    /// Rows claimed at once.
    pub batch: u32,
    /// How long a report that matches no watch waits before it expires.
    pub parked_ttl: Duration,
    /// Claims of one row before it is given up on. A row that keeps failing retryably, or keeps
    /// crashing its worker (its lease lapses), ends here. A claim handed back at shutdown, or one
    /// that only parked the row, is not counted ([`InboxItem::counted_attempts`]).
    pub max_attempts: u32,
    /// First retry delay; doubles per attempt.
    pub backoff_base: Duration,
    /// Cap of the retry delay.
    pub backoff_max: Duration,
}

/// Default of [`InboxConfig::lease`], seconds (`INBOX_LEASE_SECS`).
pub const DEFAULT_LEASE_SECS: u64 = 30;
/// Default of [`InboxConfig::poll_interval`], seconds (`INBOX_POLL_SECS`).
pub const DEFAULT_POLL_SECS: u64 = 2;
/// Default of [`InboxConfig::parked_ttl`], seconds (`INBOX_PARKED_TTL_SECS`): one day.
pub const DEFAULT_PARKED_TTL_SECS: u64 = 86_400;
/// Default of [`InboxConfig::max_attempts`] (`INBOX_MAX_ATTEMPTS`).
pub const DEFAULT_MAX_ATTEMPTS: u32 = 10;

impl Default for InboxConfig {
    fn default() -> Self {
        InboxConfig {
            lease: Duration::from_secs(DEFAULT_LEASE_SECS),
            poll_interval: Duration::from_secs(DEFAULT_POLL_SECS),
            batch: 16,
            parked_ttl: Duration::from_secs(DEFAULT_PARKED_TTL_SECS),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
        }
    }
}

/// Why a row could not be applied.
enum Failure {
    /// Trying again cannot help: the row is given up on.
    Permanent(String),
    /// Trying again later may help.
    Retryable(String),
}

impl Failure {
    fn of(e: &AppError) -> Self {
        let text = report(e);
        if e.is_retryable() {
            Failure::Retryable(text)
        } else {
            Failure::Permanent(text)
        }
    }
}

fn add(ts: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| ts.checked_add(d).ok())
        .unwrap_or(ts)
}

fn sub(ts: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| ts.checked_sub(d).ok())
        .unwrap_or(ts)
}

/// The durable inbox worker. See the module documentation.
pub struct InboxWorker<P: Ports> {
    app: Arc<App<P>>,
    cfg: InboxConfig,
    owner: String,
}

impl<P: Ports> InboxWorker<P> {
    /// `owner` identifies this process in leases (unique per replica).
    pub fn new(app: Arc<App<P>>, cfg: InboxConfig, owner: impl Into<String>) -> Arc<Self> {
        Arc::new(InboxWorker {
            app,
            cfg,
            owner: owner.into(),
        })
    }

    fn store(&self) -> &P::Store {
        self.app.ports().store()
    }

    fn now(&self) -> Timestamp {
        self.app.ports().clock().now()
    }

    /// Runs until `shutdown` is cancelled: finishes the row it is on, releases its leases so
    /// another replica takes the rest at once, and returns. Dropping (aborting) this future
    /// stops it at once, which is what a crashed process looks like.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        // Subscribe before the first claim, so a row received in between is not missed.
        let mut wake = self.app.ports().wakeup().subscribe();
        let mut wake_open = true;
        while !shutdown.is_cancelled() {
            let full = self.step(&shutdown).await >= self.cfg.batch as usize;
            if full {
                continue;
            }
            let tick = tokio::time::sleep(self.cfg.poll_interval);
            tokio::pin!(tick);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    topic = wake.next(), if wake_open => match topic {
                        Some(Topic::Inbox | Topic::Resync) => break,
                        Some(Topic::Thread(_) | Topic::Outbox) => {}
                        None => wake_open = false,
                    },
                    () = &mut tick => break,
                }
            }
        }
        if let Err(e) = self
            .store()
            .release_inbox_leases(&self.owner, self.now())
            .await
        {
            tracing::warn!(error = %report(&e), "releasing inbox leases failed");
        }
    }

    /// One pass: expires the parked rows that outlived their time-to-live, claims what is due
    /// (at most one batch) and applies it, oldest first. Returns how many rows it claimed.
    /// [`run`](Self::run) is this in a loop; tests call it to drive the worker one step at a
    /// time, with no timing involved.
    pub async fn tick(&self) -> usize {
        self.step(&CancellationToken::new()).await
    }

    async fn step(&self, shutdown: &CancellationToken) -> usize {
        self.expire_parked().await;
        let claimed = match self
            .store()
            .claim_inbox(&self.owner, self.now(), self.cfg.lease, self.cfg.batch)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %report(&e), "claiming inbox rows failed");
                Vec::new()
            }
        };
        let claimed_count = claimed.len();
        for row in claimed {
            if shutdown.is_cancelled() {
                break;
            }
            let span = tracing::info_span!(
                "inbox",
                id = %row.id,
                kind = %row.kind,
                source = %row.source,
                attempt = row.attempts,
            );
            self.process(row).instrument(span).await;
        }
        claimed_count
    }

    /// Parked rows older than the time-to-live become `expired`.
    async fn expire_parked(&self) {
        let now = self.now();
        match self
            .store()
            .expire_parked_inbox(sub(now, self.cfg.parked_ttl), now)
            .await
        {
            Ok(0) => {}
            Ok(n) => tracing::warn!(
                expired = n,
                "reports matched no watch within the parked time-to-live and expired"
            ),
            Err(e) => tracing::warn!(error = %report(&e), "expiring parked inbox rows failed"),
        }
    }

    async fn process(&self, row: InboxItem) {
        let Some(lease) = row.lease() else {
            tracing::error!("a claimed inbox row carries no lease");
            return;
        };
        let counted = row.counted_attempts();
        if counted > self.cfg.max_attempts {
            // Earlier claims ended without an answer: the worker died with the row in hand (a
            // panic, the kernel's OOM killer), which no error path here can see. Delivering it
            // once more would only kill the next worker too.
            self.dead_letter(
                &lease,
                format!(
                    "gave up: {} earlier claims ended without an answer (the worker died \
                     with the row, or lost its lease)",
                    counted - 1
                ),
            )
            .await;
            return;
        }
        let outcome = match self.deliver(&row, &lease).await {
            Ok(()) => return,
            Err(failure) => failure,
        };
        let now = self.now();
        let given_up = match outcome {
            Failure::Permanent(error) => Some(error),
            Failure::Retryable(error) if counted >= self.cfg.max_attempts => {
                Some(format!("gave up after {counted} attempts: {error}"))
            }
            Failure::Retryable(error) => {
                let after = self.backoff(counted);
                tracing::warn!(%error, ?after, "inbox row failed; it will be retried");
                match self
                    .store()
                    .retry_inbox(&lease, add(now, after), error)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => tracing::warn!("lease lost while scheduling a retry"),
                    Err(e) => tracing::warn!(
                        error = %report(&e),
                        "scheduling a retry failed; the lease will lapse and the row be claimed again"
                    ),
                }
                None
            }
        };
        if let Some(error) = given_up {
            self.dead_letter(&lease, error).await;
        }
    }

    /// Gives up on the claimed row for good.
    async fn dead_letter(&self, lease: &InboxLease, error: String) {
        tracing::error!(%error, "inbox row given up on");
        match self
            .store()
            .complete_inbox(lease, InboxFinal::Dead { error }, self.now())
            .await
        {
            Ok(true) => {}
            Ok(false) => tracing::warn!("lease lost while giving up on the row"),
            Err(e) => tracing::warn!(error = %report(&e), "giving up on the row failed"),
        }
    }

    fn backoff(&self, attempts: u32) -> Duration {
        let factor = 1u32
            .checked_shl(attempts.saturating_sub(1))
            .unwrap_or(u32::MAX);
        self.cfg
            .backoff_base
            .saturating_mul(factor)
            .min(self.cfg.backoff_max)
    }

    /// Turns the row into an input of its thread and applies it, or parks it.
    async fn deliver(&self, row: &InboxItem, lease: &InboxLease) -> Result<(), Failure> {
        let payload = row
            .decode()
            .map_err(|e| Failure::Permanent(e.to_string()))?;
        let (thread, input) = match payload {
            // A timer names its thread: no watch needed.
            InboxPayload::Timer { thread, timer } => (thread, Input::TimerFired(timer)),
            InboxPayload::CiReport(report) => {
                let Some(key) = row.correlation.as_deref() else {
                    return Err(Failure::Permanent(
                        "a CI report without a correlation cannot be matched".to_owned(),
                    ));
                };
                match self.store().get_watch(key).await {
                    Ok(Some(thread)) => (thread, Input::CiReported(report)),
                    Ok(None) => return self.park(lease).await,
                    Err(e) => return Err(Failure::of(&AppError::from(e))),
                }
            }
        };
        self.apply(thread, input, lease).await
    }

    /// No thread watches this row's correlation yet: it waits for one.
    async fn park(&self, lease: &InboxLease) -> Result<(), Failure> {
        match self.store().park_inbox(lease, self.now()).await {
            Ok(Parking::Parked) => tracing::debug!("no watch yet; the report is parked"),
            Ok(Parking::Rearmed) => tracing::debug!("the watch appeared while parking; retrying"),
            Ok(Parking::Lost) => tracing::warn!("lease lost while parking"),
            Err(e) => return Err(park_failure(&e)),
        }
        Ok(())
    }

    async fn apply(
        &self,
        thread: ThreadId,
        input: Input,
        lease: &InboxLease,
    ) -> Result<(), Failure> {
        match self.app.apply_from_inbox(thread, input, lease).await {
            Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => Ok(()),
            Ok(ApplyOutcome::Fenced) => {
                tracing::warn!("lease lost; the row belongs to another worker now");
                Ok(())
            }
            // The thread was deleted (ADR 0043) between the row's claim and its commit: drop the
            // row, with a line and a counter. It is finished, never retried, never dead-lettered
            // as a failure; a timer of the thread was deleted with it, and this is the race.
            Err(AppError::NotFound | AppError::Store(StoreError::NotFound)) => {
                self.app.late_input_dropped(LateSource::Inbox, thread);
                match self
                    .store()
                    .complete_inbox(lease, InboxFinal::Applied, self.now())
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => tracing::debug!("the row was gone with the thread"),
                    Err(e) => tracing::warn!(
                        error = %report(&e),
                        "finishing the row of a deleted thread failed; its lease will lapse"
                    ),
                }
                Ok(())
            }
            Err(e) => Err(Failure::of(&e)),
        }
    }
}

fn park_failure(e: &StoreError) -> Failure {
    let text = report(e);
    if e.is_retryable() {
        Failure::Retryable(text)
    } else {
        Failure::Permanent(text)
    }
}
