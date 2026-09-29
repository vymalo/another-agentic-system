//! The durable dispatcher: claims outbox rows under a lease, delegates to the agent, and turns
//! everything the agent reports into events through [`App::apply`].
//!
//! Crash safety rests on four facts:
//! - the outbox row is written in the same transaction as the user's message;
//! - a claim is a lease; a dead worker's row becomes claimable again when the lease expires;
//! - `sent_at` (set with the first envelope) tells a re-claiming worker to *resume* the task
//!   (`resubscribe`, then `get_task` polling) instead of sending the message again;
//! - every stored agent update carries an idempotency key, so replays never duplicate events.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use jiff::{SignedDuration, Timestamp};
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, Classify, Input, ThreadId, ThreadState, TransitionError,
    report,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, BindingUpdate, Clock,
    IdemKey, Lease, OutboxFinal, OutboxItem, OutboxKind, OutboxPayload, Ports, SendRequest,
    StoreError, TaskHandle, TaskSnapshot, ThreadStore, Topic, Wakeup,
};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::{App, AppError, ApplyOutcome};

/// Tunables of the dispatcher.
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    /// Rows processed at the same time.
    pub concurrency: usize,
    /// How long a claim lasts without renewal.
    pub lease: Duration,
    /// How often a worker renews its lease.
    pub heartbeat: Duration,
    /// Safety poll of the outbox when no wakeup arrives.
    pub poll_interval: Duration,
    /// Send attempts before a delegation is dead-lettered.
    pub max_attempts: u32,
    /// First retry delay; doubles per attempt.
    pub backoff_base: Duration,
    /// Cap of the retry delay.
    pub backoff_max: Duration,
    /// First delay when polling `get_task`.
    pub poll_min: Duration,
    /// Cap of the polling delay.
    pub poll_max: Duration,
    /// Consecutive failed polls before giving up.
    pub max_poll_failures: u32,
    /// Attempts of a cancel row.
    pub max_cancel_attempts: u32,
    /// Delay before re-checking a cancel that raced a delegation.
    pub cancel_retry_delay: Duration,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        DispatcherConfig {
            concurrency: 32,
            lease: Duration::from_secs(30),
            heartbeat: Duration::from_secs(10),
            poll_interval: Duration::from_secs(2),
            max_attempts: 5,
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            poll_min: Duration::from_secs(1),
            poll_max: Duration::from_secs(10),
            max_poll_failures: 10,
            max_cancel_attempts: 10,
            cancel_retry_delay: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum DispatchError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    App(#[from] AppError),
    /// A write was refused because the row was claimed again: the late result is dropped.
    #[error("lease lost")]
    Fenced,
}

type Done = Result<(), DispatchError>;

/// How consuming a stream ended.
enum Flow {
    /// The turn ended: terminal or waiting for the user.
    Reached,
    /// The stream broke after the message was recorded as sent.
    Disconnected,
    /// The lease is gone; stop touching the thread.
    Lost,
    /// The stream failed before the message was recorded as sent.
    Failed(AgentError),
}

/// A row's thread and binding, loaded once at the start of processing.
struct Loaded {
    ctx: Ctx,
    state: ThreadState,
    binding: orch_ports::AgentBinding,
}

/// Everything the workers need to know about the delegation they serve.
struct Ctx {
    row: OutboxItem,
    /// The claim every write of this delegation is fenced with.
    lease: Lease,
    thread: ThreadId,
    agent: AgentId,
    endpoint: AgentEndpoint,
    revision: Option<String>,
}

fn env_state(env: &AgentEnvelope) -> Option<AgentTaskState> {
    env.task_state.or(match &env.update {
        Some(AgentUpdate::Status { state, .. }) => Some(*state),
        Some(AgentUpdate::Artifact { .. } | AgentUpdate::Message { .. }) | None => None,
    })
}

fn add(ts: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| ts.checked_add(d).ok())
        .unwrap_or(ts)
}

/// The durable dispatcher. See the module documentation.
pub struct Dispatcher<P: Ports> {
    app: Arc<App<P>>,
    cfg: DispatcherConfig,
    owner: String,
}

impl<P: Ports> Dispatcher<P> {
    /// `owner` identifies this process in leases (unique per replica).
    pub fn new(app: Arc<App<P>>, cfg: DispatcherConfig, owner: impl Into<String>) -> Arc<Self> {
        Arc::new(Dispatcher {
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

    /// Runs until `shutdown` is cancelled, then stops the workers and releases their leases so
    /// another replica takes over immediately. Dropping (aborting) this future stops all
    /// workers at once, which is what a crashed process looks like.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        let mut wake = self.app.ports().wakeup().subscribe();
        let mut wake_open = true;
        let mut workers: JoinSet<()> = JoinSet::new();
        loop {
            while workers.try_join_next().is_some() {}
            let free = self.cfg.concurrency.saturating_sub(workers.len());
            if free > 0 {
                let limit = u32::try_from(free.min(16)).unwrap_or(16);
                match self
                    .store()
                    .claim_outbox(&self.owner, self.now(), self.cfg.lease, limit)
                    .await
                {
                    Ok(rows) => {
                        for row in rows {
                            let this = Arc::clone(&self);
                            let token = shutdown.child_token();
                            // Every log line of the row's processing, including the ones the
                            // agent adapter emits, carries the row (the task is spawned, so a
                            // span entered here would not follow it).
                            let span = tracing::info_span!(
                                "outbox",
                                id = %row.id,
                                thread = %row.thread_id,
                                kind = ?row.kind,
                                attempt = row.attempts,
                            );
                            workers.spawn(
                                async move { this.worker(row, token).await }.instrument(span),
                            );
                        }
                    }
                    Err(e) => tracing::warn!(error = %report(&e), "claiming outbox rows failed"),
                }
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                topic = wake.next(), if wake_open => match topic {
                    Some(Topic::Outbox | Topic::Resync) => {}
                    Some(Topic::Thread(_)) => continue,
                    None => wake_open = false,
                },
                () = tokio::time::sleep(self.cfg.poll_interval) => {}
                Some(_) = workers.join_next(), if !workers.is_empty() => {}
            }
        }
        while workers.join_next().await.is_some() {}
        if let Err(e) = self.store().release_leases(&self.owner, self.now()).await {
            tracing::warn!(error = %report(&e), "releasing leases failed");
        }
    }

    /// The claim under which `row` was handed to this dispatcher.
    fn lease(&self, row: &OutboxItem) -> Lease {
        Lease {
            id: row.id,
            owner: self.owner.clone(),
            attempt: row.attempts,
        }
    }

    async fn worker(self: Arc<Self>, row: OutboxItem, token: CancellationToken) {
        let id = row.id;
        let attempt = row.attempts;
        let lease = self.lease(&row);
        tokio::select! {
            () = token.cancelled() => tracing::debug!(%id, "worker stopped by shutdown"),
            () = self.heartbeat(&lease) => tracing::warn!(%id, "lost the lease; worker stopped"),
            result = self.process(row) => match result {
                Ok(()) => {}
                Err(DispatchError::Fenced) => tracing::warn!(%id, attempt, "lease lost; the late agent result was dropped"),
                Err(e) => tracing::error!(%id, error = %report(&e), "outbox row failed; its lease will lapse and it will be retried"),
            },
        }
        if let Err(e) = self.app.ports().wakeup().notify(Topic::Outbox).await {
            tracing::debug!(error = %report(&e), "wakeup notify failed");
        }
    }

    /// Renews the lease until it is lost; returns only then.
    async fn heartbeat(&self, lease: &Lease) {
        loop {
            tokio::time::sleep(self.cfg.heartbeat).await;
            let until = add(self.now(), self.cfg.lease);
            match self.store().renew_lease(lease, until).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(e) => {
                    tracing::warn!(id = %lease.id, error = %report(&e), "lease renewal failed")
                }
            }
        }
    }

    async fn process(&self, row: OutboxItem) -> Done {
        match row.kind {
            OutboxKind::Delegate => self.delegate(row).await,
            OutboxKind::Cancel => self.cancel(row).await,
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

    /// Finishes the row. A `false` means the lease was lost: someone else owns it now.
    async fn finish(&self, row: &OutboxItem, outcome: OutboxFinal) -> Done {
        if !self
            .store()
            .complete_outbox(&self.lease(row), outcome, self.now())
            .await?
        {
            tracing::warn!(id = %row.id, "row was no longer leased to us when finishing");
        }
        Ok(())
    }

    async fn retry(&self, row: &OutboxItem, after: Duration, error: String) -> Done {
        let next = add(self.now(), after);
        if !self
            .store()
            .retry_outbox(&self.lease(row), next, error)
            .await?
        {
            tracing::warn!(id = %row.id, "row was no longer leased to us when scheduling a retry");
        }
        Ok(())
    }

    /// Applies an input under the row's lease, treating a late/replayed update for a finished
    /// thread as a no-op. A lost lease is [`DispatchError::Fenced`].
    async fn apply_quiet(&self, row: &OutboxItem, input: Input, key: String) -> Done {
        let lease = self.lease(row);
        match self
            .app
            .apply(row.thread_id, input, Some(key), None, Some(&lease))
            .await
        {
            Ok(ApplyOutcome::Fenced) => Err(DispatchError::Fenced),
            Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => Ok(()),
            Err(AppError::Transition(TransitionError::InvalidInState { state, input })) => {
                tracing::debug!(?state, input, "dropped late input");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    // ---------------------------------------------------------------- delegate

    async fn load(&self, row: &OutboxItem) -> Result<Option<Loaded>, DispatchError> {
        let Some(thread) = self.store().get_thread(None, row.thread_id).await? else {
            self.finish(row, OutboxFinal::Skipped).await?;
            return Ok(None);
        };
        let binding = self
            .store()
            .get_binding(row.thread_id)
            .await?
            .ok_or_else(|| StoreError::corrupt("binding missing"))?;
        let Some(entry) = self.app.directory().get(&binding.agent_id) else {
            self.apply_quiet(
                row,
                Input::DeliveryFailed {
                    reason: format!("agent '{}' is no longer configured", binding.agent_id),
                    retryable: false,
                },
                format!("dead:{}", row.id),
            )
            .await?;
            self.finish(
                row,
                OutboxFinal::Dead {
                    error: "agent not configured".to_owned(),
                },
            )
            .await?;
            return Ok(None);
        };
        let ctx = Ctx {
            row: row.clone(),
            lease: self.lease(row),
            thread: row.thread_id,
            agent: binding.agent_id.clone(),
            endpoint: entry.endpoint.clone(),
            revision: binding.revision.clone(),
        };
        Ok(Some(Loaded {
            ctx,
            state: thread.state,
            binding,
        }))
    }

    async fn delegate(&self, row: OutboxItem) -> Done {
        let OutboxPayload::Delegate { text, release } = row.payload.clone() else {
            return self
                .finish(
                    &row,
                    OutboxFinal::Dead {
                        error: "payload does not match kind".to_owned(),
                    },
                )
                .await;
        };
        let Some(Loaded {
            ctx,
            state,
            binding,
        }) = self.load(&row).await?
        else {
            return Ok(());
        };

        if state.is_terminal() {
            if row.sent_at.is_some() {
                return self.finish(&row, OutboxFinal::Delivered).await;
            }
            self.apply_quiet(
                &row,
                Input::DeliveryFailed {
                    reason: "message not delivered: thread already finished".to_owned(),
                    retryable: false,
                },
                format!("skipped:{}", row.id),
            )
            .await?;
            return self.finish(&row, OutboxFinal::Skipped).await;
        }

        if let Some(task_id) = row.sent_at.and(binding.task_id.clone()) {
            return self.follow_task(&ctx, task_id).await;
        }
        if row.sent_at.is_some() {
            return Err(StoreError::corrupt("sent delegation without a task id").into());
        }

        // A previous attempt may have reached the agent before we crashed or lost the lease.
        if row.attempts > 1
            && let Ok(Some(task_id)) = self
                .app
                .ports()
                .agents()
                .find_task_by_message(&ctx.endpoint, &binding.context_id, &row.id.to_string())
                .await
        {
            let update = BindingUpdate {
                task_id: Some(task_id.clone()),
                ..BindingUpdate::default()
            };
            if !self
                .store()
                .mark_sent(&ctx.lease, update, self.now())
                .await?
            {
                return Ok(());
            }
            return self.follow_task(&ctx, task_id).await;
        }

        // Continue the previous task only if it is waiting for the user.
        let continues = binding.task_id.clone().filter(|_| {
            binding
                .task_state
                .is_some_and(AgentTaskState::is_interrupted)
        });
        let req = SendRequest {
            endpoint: ctx.endpoint.clone(),
            message_id: row.id.to_string(),
            context_id: binding.context_id.clone(),
            task_id: continues.clone(),
            text,
            release,
        };
        match self.app.ports().agents().send_stream(req).await {
            Ok(stream) => {
                let guard_stale = continues.is_some();
                match self.consume(&ctx, stream, true, guard_stale).await? {
                    Flow::Reached => self.finish(&ctx.row, OutboxFinal::Delivered).await,
                    Flow::Lost => Ok(()),
                    Flow::Disconnected => {
                        let task = self
                            .store()
                            .get_binding(row.thread_id)
                            .await?
                            .and_then(|b| b.task_id)
                            .ok_or_else(|| AppError::internal("task id missing after send"))?;
                        self.follow_task(&ctx, task).await
                    }
                    Flow::Failed(e) => self.send_failed(&ctx.row, e).await,
                }
            }
            Err(e) => self.send_failed(&ctx.row, e).await,
        }
    }

    /// How long to wait before the next attempt: the backoff curve, or what the agent asked
    /// for when that is longer.
    fn delay(&self, attempts: u32, err: &AgentError) -> Duration {
        self.backoff(attempts)
            .max(err.retry_after().unwrap_or_default())
    }

    /// A delegation failed. The operator-facing texts (outbox `last_error`, the dead row) get
    /// the whole chain; the chat log gets only what the agent said about the request or fixed
    /// text for the class, never transport text.
    async fn send_failed(&self, row: &OutboxItem, err: AgentError) -> Done {
        let retryable = err.is_retryable();
        let operator = report(&err);
        if retryable && row.attempts < self.cfg.max_attempts {
            tracing::warn!(id = %row.id, attempt = row.attempts, error = %operator, class = ?err.class(), "delegation failed; will retry");
            return self
                .retry(row, self.delay(row.attempts, &err), operator)
                .await;
        }
        tracing::warn!(id = %row.id, error = %operator, class = ?err.class(), retryable, "delegation dead-lettered");
        self.give_up(row, err.public_detail(), operator, retryable)
            .await
    }

    /// Resumes a task whose message was already sent: resubscribe, else poll `get_task`.
    async fn follow_task(&self, ctx: &Ctx, task_id: String) -> Done {
        let handle = TaskHandle {
            endpoint: ctx.endpoint.clone(),
            task_id,
        };
        match self.app.ports().agents().resubscribe(&handle).await {
            Ok(stream) => match self.consume(ctx, stream, false, false).await? {
                Flow::Reached => return self.finish(&ctx.row, OutboxFinal::Delivered).await,
                Flow::Lost => return Ok(()),
                Flow::Disconnected | Flow::Failed(_) => {}
            },
            Err(e) => {
                tracing::debug!(id = %ctx.row.id, error = %report(&e), "resubscribe unavailable; polling")
            }
        }
        self.poll_task(ctx, &handle).await
    }

    /// Polls `get_task` with backoff until the task ends its turn.
    async fn poll_task(&self, ctx: &Ctx, handle: &TaskHandle) -> Done {
        let mut delay = self.cfg.poll_min;
        let mut failures = 0u32;
        loop {
            match self.app.ports().agents().get_task(handle).await {
                Ok(snap) => {
                    failures = 0;
                    let ends = snap.state.ends_turn();
                    self.apply_snapshot(ctx, &snap).await?;
                    if ends {
                        return self.finish(&ctx.row, OutboxFinal::Delivered).await;
                    }
                }
                Err(AgentError::TaskNotFound(m)) => {
                    let reason = format!("agent lost the task: {m}");
                    return self.give_up(&ctx.row, reason.clone(), reason, false).await;
                }
                Err(e) if e.is_retryable() => {
                    failures += 1;
                    tracing::warn!(id = %ctx.row.id, failures, error = %report(&e), "polling the task failed");
                    if failures >= self.cfg.max_poll_failures {
                        return self
                            .give_up(&ctx.row, e.public_detail(), report(&e), true)
                            .await;
                    }
                }
                Err(e) => {
                    return self
                        .give_up(&ctx.row, e.public_detail(), report(&e), false)
                        .await;
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(self.cfg.poll_max);
        }
    }

    /// Dead-letters the row: `reason` is what the thread's users see, `operator` (the whole
    /// error chain) is what the outbox row keeps.
    async fn give_up(
        &self,
        row: &OutboxItem,
        reason: String,
        operator: String,
        retryable: bool,
    ) -> Done {
        self.apply_quiet(
            row,
            Input::DeliveryFailed { reason, retryable },
            format!("dead:{}", row.id),
        )
        .await?;
        self.finish(row, OutboxFinal::Dead { error: operator })
            .await
    }

    /// Consumes envelopes until the turn ends. With `mark_first`, the first envelope records
    /// the message as sent (and the task id) before anything else happens.
    ///
    /// `guard_stale`: when continuing an `input-required` task, an `input-required` envelope
    /// seen before the task moved to `working` is a stale snapshot, not the answer.
    async fn consume(
        &self,
        ctx: &Ctx,
        mut stream: AgentStream,
        mark_first: bool,
        guard_stale: bool,
    ) -> Result<Flow, DispatchError> {
        let mut marked = !mark_first;
        let mut seen_working = !guard_stale;
        loop {
            match stream.next().await {
                Some(Ok(env)) => {
                    if !marked {
                        let update = BindingUpdate {
                            task_id: Some(env.task_id.clone()),
                            task_state: env_state(&env),
                            revision: env.revision.clone(),
                        };
                        if !self
                            .store()
                            .mark_sent(&ctx.lease, update, self.now())
                            .await?
                        {
                            return Ok(Flow::Lost);
                        }
                        marked = true;
                    }
                    let state = env_state(&env);
                    if state.is_some_and(|s| !s.is_interrupted()) {
                        seen_working = true;
                    }
                    if !seen_working && state.is_some_and(AgentTaskState::is_interrupted) {
                        continue;
                    }
                    self.apply_envelope(ctx, &env).await?;
                    if state.is_some_and(AgentTaskState::ends_turn) {
                        return Ok(Flow::Reached);
                    }
                }
                Some(Err(e)) => {
                    return Ok(if marked {
                        Flow::Disconnected
                    } else {
                        Flow::Failed(e)
                    });
                }
                None => {
                    return Ok(if marked {
                        Flow::Disconnected
                    } else {
                        Flow::Failed(AgentError::protocol("stream ended before the first update"))
                    });
                }
            }
        }
    }

    async fn apply_envelope(&self, ctx: &Ctx, env: &AgentEnvelope) -> Done {
        let key = match &env.key {
            IdemKey::Task(k) => k.clone(),
            IdemKey::Turn(k) => format!("turn:{}:{k}", ctx.row.id),
        };
        let binding = BindingUpdate {
            task_id: Some(env.task_id.clone()),
            task_state: env_state(env),
            revision: env.revision.clone(),
        };
        match &env.update {
            Some(update) => {
                let input = Input::Agent {
                    agent: ctx.agent.clone(),
                    revision: env.revision.clone().or_else(|| ctx.revision.clone()),
                    update: update.clone(),
                };
                match self
                    .app
                    .apply(
                        ctx.thread,
                        input,
                        Some(key),
                        Some(binding),
                        Some(&ctx.lease),
                    )
                    .await
                {
                    Ok(ApplyOutcome::Fenced) => Err(DispatchError::Fenced),
                    Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => Ok(()),
                    Err(AppError::Transition(TransitionError::InvalidInState { state, input })) => {
                        tracing::debug!(?state, input, "dropped late agent update");
                        Ok(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            None => match self
                .app
                .record_binding(ctx.thread, binding, Some(&ctx.lease))
                .await?
            {
                ApplyOutcome::Fenced => Err(DispatchError::Fenced),
                ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate => Ok(()),
            },
        }
    }

    /// Applies a polled snapshot; if it lacks a status envelope for its own state, one is
    /// synthesised so the thread never misses the end of the turn.
    async fn apply_snapshot(&self, ctx: &Ctx, snap: &TaskSnapshot) -> Done {
        for env in &snap.envelopes {
            self.apply_envelope(ctx, env).await?;
        }
        let covered = snap
            .envelopes
            .iter()
            .any(|e| env_state(e) == Some(snap.state));
        if !covered {
            let env = AgentEnvelope {
                task_id: snap.task_id.clone(),
                context_id: snap.context_id.clone(),
                task_state: Some(snap.state),
                revision: snap.revision.clone(),
                key: IdemKey::Turn(format!("{}:status:{:?}", snap.task_id, snap.state)),
                update: Some(AgentUpdate::Status {
                    state: snap.state,
                    detail: None,
                }),
            };
            self.apply_envelope(ctx, &env).await?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ cancel

    async fn cancel(&self, row: OutboxItem) -> Done {
        let Some(Loaded {
            ctx,
            state,
            binding,
        }) = self.load(&row).await?
        else {
            return Ok(());
        };
        if state.is_terminal() {
            return self.finish(&row, OutboxFinal::Delivered).await;
        }

        if let Some(task_id) = binding.task_id {
            let handle = TaskHandle {
                endpoint: ctx.endpoint.clone(),
                task_id,
            };
            return match self.app.ports().agents().cancel(&handle).await {
                Ok(snap) => {
                    self.apply_snapshot(&ctx, &snap).await?;
                    self.finish(&row, OutboxFinal::Delivered).await
                }
                Err(AgentError::NotCancelable(reason) | AgentError::TaskNotFound(reason)) => {
                    self.reject_cancel(&row, reason, false, OutboxFinal::Delivered)
                        .await
                }
                Err(e) if e.is_retryable() && row.attempts < self.cfg.max_cancel_attempts => {
                    self.retry(&row, self.delay(row.attempts, &e), report(&e))
                        .await
                }
                Err(e) => {
                    let retryable = e.is_retryable();
                    self.reject_cancel(
                        &row,
                        e.public_detail(),
                        retryable,
                        OutboxFinal::Dead { error: report(&e) },
                    )
                    .await
                }
            };
        }

        // Nothing reached the agent yet: cancel whatever has not been sent.
        let skipped = self
            .store()
            .skip_unsent_delegates(row.thread_id, self.now())
            .await?;
        let in_flight = self
            .store()
            .list_open_outbox(row.thread_id)
            .await?
            .iter()
            .filter(|r| r.kind == OutboxKind::Delegate)
            .count();
        if skipped > 0 || in_flight == 0 {
            self.apply_quiet(
                &row,
                Input::CancelledBeforeStart,
                format!("cancelstart:{}", row.id),
            )
            .await?;
            return self.finish(&row, OutboxFinal::Delivered).await;
        }
        // A delegation is being sent right now; once it has a task id the next attempt cancels it.
        if row.attempts < self.cfg.max_cancel_attempts {
            return self
                .retry(
                    &row,
                    self.cfg.cancel_retry_delay,
                    "delegation in flight".to_owned(),
                )
                .await;
        }
        self.reject_cancel(
            &row,
            "delegation still in flight".to_owned(),
            true,
            OutboxFinal::Dead {
                error: "delegation still in flight".to_owned(),
            },
        )
        .await
    }

    async fn reject_cancel(
        &self,
        row: &OutboxItem,
        reason: String,
        retryable: bool,
        outcome: OutboxFinal,
    ) -> Done {
        self.apply_quiet(
            row,
            Input::CancelRejected { reason, retryable },
            format!("cancelrej:{}", row.id),
        )
        .await?;
        self.finish(row, outcome).await
    }
}
