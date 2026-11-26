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
    AccessTier, AgentId, AgentTaskState, AgentUpdate, Classify, Event, ForkHistory, Input, Mention,
    SessionNews, ThreadId, ThreadState, ToolsGrant, TransitionError, UsageUpdate, UserId,
    fork_history, report,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, BindingUpdate, ClaimOrder,
    Clock, IdemKey, Lease, NoRunBindings, OutboxFinal, OutboxItem, OutboxKind, OutboxPayload,
    Ports, RunBinding, RunBindingError, RunBindings, RunId, SendContent, SendRequest, StoreError,
    TaskHandle, TaskSnapshot, ThreadStore, Topic, Wakeup,
};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::app::sandbox_rejection;
use crate::{App, AppError, ApplyOutcome, ChildSessionDrop, LateSource, UsageDrop};

mod ask;
mod description;
mod files;
mod live;
mod sessions;
mod title;
mod utility;
mod verify;

/// Events read at a time when a fork's history is built.
const FORK_PAGE: u32 = 500;

/// How long a steer waits for the agent to answer with the task (`steer/v1`): the first event of
/// the stream. A steer that is not answered by then is not delivered, and is tried again or falls
/// back like any other failure.
const STEER_ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

use live::{LiveRelay, LiveTiming};

pub use files::{FileLimits, MAX_FILES_PER_JOB};
pub(crate) use files::{canonical_media_type, clean_filename, sniff};

/// Tunables of the dispatcher.
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    /// Rows processed at the same time.
    pub concurrency: usize,
    /// Which due rows a claim takes first when there are more than free slots (ADR 0066): by the
    /// load of their person, or oldest first.
    pub order: ClaimOrder,
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
    /// How often a verification or an ask in flight looks at its thread to see that it is still wanted
    /// (a timeout, a cancel or a message from the user ends it); a verifier that hangs is
    /// dropped within this long of that.
    pub verify_watch: Duration,
    /// The least time between two publishes of the live text of one reply (`text-stream/v1`, ADR
    /// 0027): what arrives in between is merged. The last piece of a reply goes out at once.
    pub live_flush: Duration,
    /// How often the text of a reply so far is published again from its beginning, so that a
    /// viewer that connects mid-stream, or lost a piece, has it within this long.
    pub live_refresh: Duration,
    /// The most text of one reply that is held for the refresh, in bytes; a longer reply is relayed
    /// piece by piece without one.
    pub live_max_bytes: usize,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        DispatcherConfig {
            concurrency: 32,
            order: ClaimOrder::Fair,
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
            verify_watch: Duration::from_secs(5),
            live_flush: Duration::from_millis(100),
            live_refresh: Duration::from_secs(1),
            live_max_bytes: 64 * 1024,
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
    /// The agent answered a round with a task of another context than the child thread's: nothing
    /// of it was applied (ADR 0072).
    #[error("the round's task is of another context")]
    ForeignContext,
    /// A utility call's lease could not be bound (the broker is down, or the key names another
    /// person): the row waits for its retry and the model is not asked.
    #[error("the utility call's lease could not be bound: {0}")]
    Lease(String),
}

type Done = Result<(), DispatchError>;

/// Whether the failure is the store or the application saying that the thread is not there.
fn is_not_found(e: &DispatchError) -> bool {
    matches!(
        e,
        DispatchError::Store(StoreError::NotFound)
            | DispatchError::App(AppError::NotFound | AppError::Store(StoreError::NotFound))
    )
}

/// How often a lookup by message id is tried before the row is retried (or the thread held, or the
/// ask failed): a lookup that fails says nothing about whether the agent was asked.
const FIND_TRIES: u32 = 3;

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
    /// The number of the thread's current job (ADR 0020).
    job: u32,
    /// The ids of the MCP servers attached to the thread when it was read (ADR 0024).
    tools: Vec<String>,
    binding: orch_ports::AgentBinding,
    /// The thread is a helper session's (ADR 0072): what is sent to it goes to the session's
    /// context, naming no earlier round.
    is_child: bool,
    /// The last event the thread copied from its parent, when it is a fork (ADR 0029): the
    /// events `1..=cut` are the conversation its first task is told.
    forked_at: Option<i64>,
}

/// A delegation that opens a task of its own: what [`Dispatcher::consume`] needs to tell, from the
/// first envelope, that the agent made a second task of a thread that has ended (open question 33).
struct Adopt {
    /// The words of the message.
    text: String,
    /// The agents it mentions, as its row holds them: they join the job the message is adopted
    /// into.
    mentions: Vec<Mention>,
    /// The access tier it named, as its row holds it (ADR 0061): the job it is adopted into
    /// carries it as its own message does, and the cap that went with it.
    tier: Option<AccessTier>,
    tier_cap: Option<AccessTier>,
    /// The interface locale it was written with (ADR 0085), as its row holds it: the adopted
    /// message keeps it, as it keeps its tier.
    locale: Option<orch_core::LocaleHint>,
    /// The model and reasoning effort picked for the message (ADR 0086), checked against the live card before anything is written, and carried as the tier is.
    pick: Option<orch_core::ModelPick>,
    /// The task the binding had when the message was sent, if any.
    previous: Option<String>,
}

/// Everything the workers need to know about the delegation they serve.
struct Ctx {
    row: OutboxItem,
    /// The claim every write of this delegation is fenced with.
    lease: Lease,
    thread: ThreadId,
    /// The thread's job the delegation belongs to (an upload id names the job it was issued in).
    job: u32,
    agent: AgentId,
    /// The person the thread belongs to: the run this delegation opens spends for them (ADR 0084).
    person: UserId,
    endpoint: AgentEndpoint,
    revision: Option<String>,
    /// What the delegation has kept of the agent's files so far (ADR 0032).
    files: files::Budget,
    /// The model and reasoning effort the message picked (ADR 0086), read at the bind to resolve
    /// the endpoint the live card maps it to (ADR 0084, the status note of 2026-11-24).
    pick: Option<orch_core::ModelPick>,
    /// For the row that follows a child session's round: the context the child thread adopted.
    /// Nothing of another context may be applied to the child (fail closed against an agent that
    /// names the parent's task, or another conversation's, as a round).
    expected_context: Option<String>,
}

/// What a request tells the adapter of the cap it holds: the cap only when the message goes out
/// with **no tier** because the card listed none ([`App::tier_for_send`]), so that the adapter,
/// reading the card again for this very call, refuses (retryably) a message that would reach an
/// agent that lists tiers by now (see `SendRequest::tier_cap`).
pub(super) fn plain_cap(sent: Option<AccessTier>, cap: Option<AccessTier>) -> Option<AccessTier> {
    if sent.is_none() { cap } else { None }
}

fn env_state(env: &AgentEnvelope) -> Option<AgentTaskState> {
    env.task_state.or(match &env.update {
        Some(AgentUpdate::Status { state, .. }) => Some(*state),
        Some(
            AgentUpdate::Artifact { .. }
            | AgentUpdate::Message { .. }
            | AgentUpdate::Reasoning { .. }
            | AgentUpdate::Ui { .. }
            | AgentUpdate::UiRejected { .. }
            | AgentUpdate::File { .. }
            | AgentUpdate::FileUploaded { .. }
            | AgentUpdate::FileKept { .. }
            | AgentUpdate::FileRefused { .. }
            | AgentUpdate::Step(_)
            | AgentUpdate::Usage(_)
            | AgentUpdate::UsageRejected(_)
            | AgentUpdate::TierRejected
            | AgentUpdate::ChildSessions(_)
            | AgentUpdate::ChildSessionsRejected(_)
            | AgentUpdate::TrustedRequest(_),
        )
        | None => None,
    })
}

/// What `mark_sent` records with the message: the task it reached and, while that task is not
/// over, its state. The binding records a task as over only once its end is applied to the
/// thread (`apply_envelope` commits the two together, and drops an end the binding records
/// already, ADR 0036). A task can be over before its stream says anything (a fast agent's stream
/// begins with a snapshot of the finished task): recording that end with the message would have
/// the end dropped as already applied, and the thread would wait forever. Until its envelope is
/// applied the task is `submitted`, which also replaces the state the binding kept of the task
/// before it.
///
/// The context the agent answered in is adopted with it (ADR 0055): the first message of a thread
/// named none, and the binding keeps the one the agent assigned.
fn sent_binding(
    task_id: String,
    context_id: Option<String>,
    state: Option<AgentTaskState>,
    revision: Option<String>,
) -> BindingUpdate {
    BindingUpdate {
        context_id: context_id.filter(|c| !c.is_empty()),
        task_id: Some(task_id),
        task_state: Some(
            state
                .filter(|s| !s.is_terminal())
                .unwrap_or(AgentTaskState::Submitted),
        ),
        revision,
    }
}

fn add(ts: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| ts.checked_add(d).ok())
        .unwrap_or(ts)
}

/// The longest a run's binding lives, from the moment its task id is first seen (ADR 0084
/// decision 1): the broker's own cap, because the orchestrator has no per-run deadline. A run
/// that outlives it loses its lease, and S9 parks it.
pub const RUN_BINDING_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The durable dispatcher. See the module documentation.
pub struct Dispatcher<P: Ports, B: RunBindings = NoRunBindings> {
    app: Arc<App<P>>,
    cfg: DispatcherConfig,
    owner: String,
    /// The broker that binds a run and unbinds it (ADR 0084): [`NoRunBindings`] in a deployment
    /// with none.
    run_bindings: B,
    /// The rounds of each parent's child sessions this process follows now (see
    /// [`MAX_OBSERVED_PER_PARENT`]).
    observing: std::sync::Mutex<std::collections::HashMap<ThreadId, usize>>,
}

/// The most rounds of one parent's child sessions a dispatcher follows at once (ADR 0072). Each
/// followed round holds a worker for as long as the round runs, and an agent chooses how many
/// sessions it opens, so without a bound one agent could take every worker of the process. A row
/// beyond it **waits** (it is tried again with backoff and never dropped or dead-lettered for it).
pub const MAX_OBSERVED_PER_PARENT: usize = 8;

/// One followed round of a parent, counted until it is dropped (a finished, failed or aborted
/// worker alike).
struct ObservingSlot<'a> {
    map: &'a std::sync::Mutex<std::collections::HashMap<ThreadId, usize>>,
    parent: ThreadId,
}

impl Drop for ObservingSlot<'_> {
    fn drop(&mut self) {
        let mut map = self
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(n) = map.get_mut(&self.parent) {
            *n -= 1;
            if *n == 0 {
                map.remove(&self.parent);
            }
        }
    }
}

impl<P: Ports> Dispatcher<P> {
    /// `owner` identifies this process in leases (unique per replica). No run is bound: the
    /// deployment has no broker ([`NoRunBindings`]).
    pub fn new(app: Arc<App<P>>, cfg: DispatcherConfig, owner: impl Into<String>) -> Arc<Self> {
        Dispatcher::with_run_bindings(app, cfg, owner, NoRunBindings)
    }
}

impl<P: Ports, B: RunBindings> Dispatcher<P, B> {
    /// [`Dispatcher::new`] with the broker that binds each run and unbinds it when it ends (ADR
    /// 0084).
    pub fn with_run_bindings(
        app: Arc<App<P>>,
        cfg: DispatcherConfig,
        owner: impl Into<String>,
        run_bindings: B,
    ) -> Arc<Self> {
        Arc::new(Dispatcher {
            app,
            cfg,
            owner: owner.into(),
            run_bindings,
            observing: std::sync::Mutex::default(),
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
            self.app
                .set_dispatcher_slots(workers.len(), self.cfg.concurrency);
            let free = self.cfg.concurrency.saturating_sub(workers.len());
            if free > 0 {
                let limit = u32::try_from(free.min(16)).unwrap_or(16);
                match self
                    .store()
                    .claim_outbox(
                        &self.owner,
                        self.now(),
                        self.cfg.lease,
                        limit,
                        self.cfg.order,
                    )
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
            self.app
                .set_dispatcher_slots(workers.len(), self.cfg.concurrency);
            tokio::select! {
                () = shutdown.cancelled() => break,
                topic = wake.next(), if wake_open => match topic {
                    Some(Topic::Outbox | Topic::Resync) => {}
                    Some(Topic::Thread(_) | Topic::Inbox) => continue,
                    None => wake_open = false,
                },
                () = tokio::time::sleep(self.cfg.poll_interval) => {}
                Some(_) = workers.join_next(), if !workers.is_empty() => {}
            }
        }
        while workers.join_next().await.is_some() {}
        self.app.set_dispatcher_slots(0, 0);
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
        let thread = row.thread_id;
        let lease = self.lease(&row);
        let kept = row.clone();
        tokio::select! {
            () = token.cancelled() => tracing::debug!(%id, "worker stopped by shutdown"),
            () = self.heartbeat(&lease) => {
                // The usual way a delete reaches a worker that is waiting on an agent: the row went
                // with the thread (ADR 0043), so the next renewal found no claim.
                if self.thread_is_gone(thread).await {
                    self.app.late_input_dropped(LateSource::Dispatcher, thread);
                } else {
                    tracing::warn!(%id, "lost the lease; worker stopped");
                }
            }
            result = Box::pin(self.process(row)) => match result {
                Ok(()) => {}
                Err(DispatchError::Fenced) => tracing::warn!(%id, attempt, "lease lost; the late agent result was dropped"),
                // A helper's message answered from another context (ADR 0072): nothing of it was
                // applied, and sending the same message again would be answered the same way, so
                // the job fails with a word instead of waiting for a lease to lapse.
                Err(DispatchError::ForeignContext) => {
                    let why = "the agent answered with a task of another context than this helper's".to_owned();
                    if let Err(e) = self.give_up(&kept, why.clone(), why, false).await {
                        tracing::error!(%id, error = %report(&e), "could not fail a row answered from a foreign context");
                    }
                }
                // The thread was deleted while the worker held a result for it (ADR 0043): drop it,
                // with a line and a counter. The row went with the thread, so nothing retries it.
                Err(e) if is_not_found(&e) && self.thread_is_gone(thread).await => {
                    self.app.late_input_dropped(LateSource::Dispatcher, thread);
                }
                Err(e) => tracing::error!(%id, error = %report(&e), "outbox row failed; its lease will lapse and it will be retried"),
            },
        }
        if let Err(e) = self.app.ports().wakeup().notify(Topic::Outbox).await {
            tracing::debug!(error = %report(&e), "wakeup notify failed");
        }
        // The thread may have reached an end state under this row (ADR 0084): its run's binding
        // goes with it, whichever path ended the job.
        self.end_run(thread).await;
    }

    /// Whether the store has no such thread any more (deleted, ADR 0043). A store that cannot
    /// say is not taken to mean it: the question is asked again by the next failure.
    async fn thread_is_gone(&self, thread: ThreadId) -> bool {
        matches!(self.store().get_thread(None, thread).await, Ok(None))
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

    /// Every arm is boxed: the futures of the paths below are large and nest (a delegation reads a
    /// stream, applies envelopes, follows sessions), so one boxed boundary per row kind keeps what
    /// a worker polls on its stack to the size of a pointer, whichever arm runs.
    async fn process(&self, row: OutboxItem) -> Done {
        match row.kind {
            // A round of a child session is followed, never sent (ADR 0072): a path of its own, so
            // that the frame of a delegation is not under it
            OutboxKind::Delegate if matches!(row.payload, OutboxPayload::Observe { .. }) => {
                Box::pin(self.observe(row)).await
            }
            OutboxKind::Delegate => Box::pin(self.delegate(row)).await,
            // A person's start and close of a helper session (ADR 0072) share the lane and are
            // never steers
            OutboxKind::Steer if matches!(row.payload, OutboxPayload::SessionStart { .. }) => {
                Box::pin(self.start_session(row)).await
            }
            OutboxKind::Steer if matches!(row.payload, OutboxPayload::SessionClose { .. }) => {
                Box::pin(self.close_session(row)).await
            }
            OutboxKind::Steer => Box::pin(self.steer(row)).await,
            OutboxKind::Cancel => Box::pin(self.cancel(row)).await,
            OutboxKind::Verify => Box::pin(self.verify(row)).await,
            OutboxKind::Title => Box::pin(self.title(row)).await,
            OutboxKind::Description => Box::pin(self.description(row)).await,
            OutboxKind::Ask => Box::pin(self.ask(row)).await,
        }
    }

    /// Looks for the task an earlier claim may have started for this row's message (its id is the
    /// row's) in `context`, trying again when the lookup itself fails. For the rows that run in a
    /// conversation of their own (`verify`, `ask`); the delegation looks once, with the thread's.
    async fn find_by_message(
        &self,
        endpoint: &AgentEndpoint,
        context: Option<&str>,
        row: &OutboxItem,
    ) -> Result<Option<String>, AgentError> {
        let mut delay = self.cfg.poll_min;
        let mut tries = 0_u32;
        loop {
            match self
                .app
                .ports()
                .agents()
                .find_task_by_message(endpoint, context, &row.id.to_string())
                .await
            {
                Ok(found) => return Ok(found),
                Err(e) if e.is_retryable() && tries + 1 < FIND_TRIES => {
                    tries += 1;
                    tracing::warn!(id = %row.id, tries, error = %report(&e), "looking for the agent's task failed");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(self.cfg.poll_max);
                }
                Err(e) => return Err(e),
            }
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

    // --------------------------------------------------------- the run's binding

    /// Binds the run whose task id the delegation's stream has just named (ADR 0084, the ruling of
    /// 2026-11-23: the bind is at the first sight of the id, before any of the delegation's events
    /// are applied). A broker that cannot be reached is logged and the run goes on; S9 parks it.
    ///
    /// The binding names the endpoint the live card maps the run's pick to ([`Self::run_endpoint`]).
    /// A later message of a run already bound that picks another alias asks for that choice's
    /// endpoint and the broker refuses it (`409`, ADR 0084, the status note of 2026-11-24): the run
    /// keeps the endpoint it was bound to, never re-pointed.
    async fn bind_run(&self, ctx: &Ctx, task: &str) {
        let binding = RunBinding {
            run: RunId::new(task),
            agent: ctx.agent.clone(),
            person: ctx.person.clone(),
            model: self.run_endpoint(ctx).await,
            expires_at: add(self.now(), RUN_BINDING_TTL),
        };
        match self.run_bindings.bind(binding).await {
            Ok(()) => tracing::debug!(run = %task, thread = %ctx.thread, "the run is bound"),
            Err(RunBindingError::Conflict) => tracing::warn!(
                run = %task,
                thread = %ctx.thread,
                "the run is already bound to another model endpoint; the pick is refused and the run keeps its own"
            ),
            Err(e) => tracing::warn!(
                run = %task,
                thread = %ctx.thread,
                error = %e,
                "binding the run failed; the run goes on (S9 parks it instead)"
            ),
        }
    }

    /// The endpoint label the run's binding names (ADR 0084, the status note of 2026-11-24): the
    /// `endpoint` of the choice the run's picked alias maps to on the **live** card, or the card's
    /// `default` choice when the message picked no model.
    ///
    /// `None` when the card names no endpoint for that choice, lists no `model-pick/v1` (or lists it
    /// badly), or cannot be read: the binding then names no model, and the lookup answers
    /// `unavailable` (fail closed).
    async fn run_endpoint(&self, ctx: &Ctx) -> Option<String> {
        let picked = ctx.pick.as_ref().and_then(|pick| pick.model.as_ref());
        match self.app.card_pick_listings(&ctx.endpoint).await {
            Ok(listings) => {
                let endpoint = listings.model.offer()?.endpoint_for(picked)?;
                Some(endpoint.to_owned())
            }
            Err(e) => {
                tracing::warn!(
                    agent = %ctx.agent,
                    thread = %ctx.thread,
                    error = %report(&e),
                    "the agent card could not be read to resolve the run's model endpoint; the binding names none"
                );
                None
            }
        }
    }

    /// Unbinds the run of `thread` once its job is over: `done`, `failed` or `cancelled` (ADR 0084
    /// decision 6). Called after every row, so that every end state follows, whichever path reached
    /// it; a run that was never bound, or one unbound already, is no error. A broker that cannot be
    /// reached is logged and the binding stays until it expires; S9 parks the run instead. A
    /// message that arrives between the end and this read moves the thread on and leaves a binding
    /// behind until it expires, which spends nothing: a run that ended makes no call.
    async fn end_run(&self, thread: ThreadId) {
        let Ok(Some(record)) = self.store().get_thread(None, thread).await else {
            return;
        };
        if !record.state.is_terminal() {
            return;
        }
        let Ok(Some(binding)) = self.store().get_binding(thread).await else {
            return;
        };
        let Some(task) = binding.task_id.filter(|task| !task.is_empty()) else {
            return;
        };
        let run = RunId::new(task.as_str());
        match self.run_bindings.unbind(&run).await {
            Ok(()) => tracing::debug!(run = %task, thread = %thread, "the run is unbound"),
            Err(e) => tracing::warn!(
                run = %task,
                thread = %thread,
                error = %e,
                "unbinding the run failed; it stays bound until it expires"
            ),
        }
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
        // The registry is read now, for every row: an agent the platform removed is dead-lettered
        // when the registry answers without it (ADR 0022), and a registry that cannot answer
        // leaves the row to be tried again, never dead.
        let entry = match self.app.resolve_agent(&binding.agent_id).await {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                let reason = format!("agent '{}' is no longer listed", binding.agent_id);
                // A person's start or close of a session is no delegation: it ends with the
                // word, and the job it was asked beside is not failed (ADR 0072)
                match &row.payload {
                    OutboxPayload::SessionStart { request, .. } => {
                        self.end_start_failed(row, request, reason.clone(), reason)
                            .await?;
                        return Ok(None);
                    }
                    OutboxPayload::SessionClose { .. } => {
                        self.close_failed(row, reason.clone(), reason).await?;
                        return Ok(None);
                    }
                    _ => {}
                }
                self.apply_quiet(
                    row,
                    Input::DeliveryFailed {
                        reason,
                        retryable: false,
                    },
                    format!("dead:{}", row.id),
                )
                .await?;
                self.finish(
                    row,
                    OutboxFinal::Dead {
                        error: "agent not listed".to_owned(),
                    },
                )
                .await?;
                return Ok(None);
            }
            Err(AppError::RegistryUnavailable { source }) => {
                tracing::warn!(id = %row.id, agent = %binding.agent_id, attempt = row.attempts, error = %report(&source), "the agent registry cannot say where the agent is; will retry");
                self.retry(
                    row,
                    self.backoff(row.attempts),
                    format!("the agent registry is unreachable: {}", report(&source)),
                )
                .await?;
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let ctx = Ctx {
            row: row.clone(),
            lease: self.lease(row),
            thread: row.thread_id,
            job: thread.job.number,
            agent: binding.agent_id.clone(),
            person: thread.owner.clone(),
            endpoint: entry.endpoint.clone(),
            revision: binding.revision.clone(),
            files: files::Budget::default(),
            // the row is the message whose pick names the run's model alias, and only a delegation
            // sends a task (a steer goes into one already bound)
            pick: match &row.payload {
                OutboxPayload::Delegate { pick, .. } => pick.clone(),
                _ => None,
            },
            // every row of a child thread, not only the follow of a round: nothing of another
            // context may be applied to a helper's thread (fail closed)
            expected_context: (thread.child_of.is_some()
                || matches!(row.payload, OutboxPayload::Observe { .. }))
            .then(|| binding.context_id.clone().unwrap_or_default()),
        };
        Ok(Some(Loaded {
            ctx,
            state: thread.state,
            job: thread.job.number,
            tools: thread.job.tools,
            binding,
            is_child: thread.child_of.is_some(),
            forked_at: thread.forked_from.map(|origin| origin.seq),
        }))
    }

    /// The conversation a fork continues, as its first task is told it: the fork's own events
    /// `1..=cut` (the copy of its parent's), read in pages, through [`fork_history`] (ADR 0029).
    ///
    /// The conversation is told **to** `agent`: it names which turns another agent said, so that
    /// an agent that takes over a fork does not answer for what the one before it said.
    async fn fork_history(
        &self,
        thread: ThreadId,
        cut: i64,
        agent: &AgentId,
    ) -> Result<ForkHistory, DispatchError> {
        let mut events: Vec<Event> = Vec::new();
        let mut after = 0;
        while after < cut {
            let page = self.store().list_events(thread, after, FORK_PAGE).await?;
            let Some(last) = page.last() else { break };
            after = last.seq;
            events.extend(page.into_iter().filter(|e| e.seq <= cut));
        }
        let directory = self.app.directory();
        let id = directory.canonical(agent);
        let aliases = directory.aliases_of(&id);
        Ok(fork_history(&events).addressed_to(&id, &aliases))
    }

    async fn delegate(&self, row: OutboxItem) -> Done {
        let (content, release, new_job, ui_catalog, mentions, tier, tier_cap, locale, pick) =
            match row.payload.clone() {
                OutboxPayload::Delegate {
                    text,
                    release,
                    new_job,
                    ui_catalog,
                    mentions,
                    tier,
                    tier_cap,
                    locale,
                    pick,
                } => (
                    SendContent::Text(text),
                    release,
                    new_job,
                    ui_catalog,
                    mentions,
                    tier,
                    tier_cap,
                    locale,
                    pick,
                ),
                OutboxPayload::Action {
                    action,
                    at,
                    release,
                    ui_catalog,
                    tier,
                    tier_cap,
                } => (
                    SendContent::UiAction { action, at },
                    release,
                    false,
                    ui_catalog,
                    Vec::new(),
                    tier,
                    tier_cap,
                    // an action is the person's act on a surface, not a message of a screen with a
                    // language (ADR 0085), and names no model (ADR 0086)
                    None,
                    None,
                ),
                // taken before this function (`process`): it is followed, never sent
                OutboxPayload::Observe { .. } => {
                    return self
                        .finish(
                            &row,
                            OutboxFinal::Dead {
                                error: "payload does not match kind".to_owned(),
                            },
                        )
                        .await;
                }
                OutboxPayload::Steer { .. }
                | OutboxPayload::SessionStart { .. }
                | OutboxPayload::SessionClose { .. }
                | OutboxPayload::Cancel { .. }
                | OutboxPayload::Verify { .. }
                | OutboxPayload::Title { .. }
                | OutboxPayload::Description { .. }
                | OutboxPayload::Ask { .. } => {
                    return self
                        .finish(
                            &row,
                            OutboxFinal::Dead {
                                error: "payload does not match kind".to_owned(),
                            },
                        )
                        .await;
                }
            };
        let Some(Loaded {
            ctx,
            state,
            job,
            tools,
            binding,
            is_child,
            forked_at,
        }) = self.load(&row).await?
        else {
            return Ok(());
        };

        if state.is_terminal() {
            if row.sent_at.is_some() {
                return self.finish(&row, OutboxFinal::Delivered).await;
            }
            // A message the person wrote while the job was open, behind a delegation that ended
            // it: the thread is a conversation (ADR 0020), so it is the next job's, not lost. A
            // person who stopped the thread asked for that; an action belongs to a finished job.
            if let OutboxPayload::Delegate {
                text,
                mentions,
                tier,
                tier_cap,
                locale,
                pick,
                ..
            } = &row.payload
                && state != ThreadState::Cancelled
            {
                // The row ends in the same commit as the job it starts: a crash between the two
                // would leave the message to be sent again on top of the redelivery.
                let lease = self.lease(&row);
                return match self
                    .app
                    .apply_finishing(
                        row.thread_id,
                        Input::Redeliver {
                            text: text.clone(),
                            sent: false,
                            mentions: mentions.clone(),
                            tier: *tier,
                            tier_cap: *tier_cap,
                            locale: locale.clone(),
                            pick: pick.clone(),
                        },
                        format!("redeliver:{}", row.id),
                        &lease,
                        OutboxFinal::Skipped,
                    )
                    .await
                {
                    Ok(ApplyOutcome::Fenced) => Err(DispatchError::Fenced),
                    Ok(ApplyOutcome::Applied { .. }) => Ok(()),
                    // written before, and its row not finished: end it now
                    Ok(ApplyOutcome::Duplicate) => self.finish(&row, OutboxFinal::Skipped).await,
                    Err(AppError::Transition(TransitionError::InvalidInState { .. })) => {
                        self.finish(&row, OutboxFinal::Skipped).await
                    }
                    Err(e) => Err(e.into()),
                };
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
                .find_task_by_message(
                    &ctx.endpoint,
                    binding.context_id.as_deref(),
                    &row.id.to_string(),
                )
                .await
        {
            let update = sent_binding(task_id.clone(), None, None, None);
            if !self
                .store()
                .mark_sent(&ctx.lease, update, self.now())
                .await?
            {
                return Ok(());
            }
            return self.follow_task(&ctx, task_id).await;
        }

        // Continue the previous task only if it is waiting for the user, and never from a new
        // job: a thread that failed while blocked keeps its `input-required` task in the
        // binding, and the next job opens a task of its own.
        let continues = binding.task_id.clone().filter(|_| {
            !new_job
                && binding
                    .task_state
                    .is_some_and(AgentTaskState::is_interrupted)
        });
        // A task that does not continue one waiting for the user is a new task of the thread (a
        // rework, a follow-up after the turn ended, the first task of a new job): it names the
        // previous task, so the agent can tell what the message is about (ADR 0021).
        // A helper session's rounds name no earlier round: the session is the context (ADR 0072).
        let reference_task_ids = match (&continues, &binding.task_id) {
            (None, Some(previous)) if !is_child => vec![previous.clone()],
            _ => Vec::new(),
        };
        // The first task of a fork has no task to continue (a fork is a context of its own), so
        // it is told the conversation it continues, which is derived from the thread's own log
        // each time the message is sent: a retry sends the same words, and nothing of it is
        // stored. A task that follows another, and an action, are told nothing of it.
        let history = match (forked_at, &binding.task_id, &content) {
            (Some(cut), None, SendContent::Text(_)) => {
                Some(self.fork_history(row.thread_id, cut, &ctx.agent).await?)
            }
            _ => None,
        };
        // The agents the message mentions, named as the registry gives them now (ADR 0026); the
        // adapter tells the agent only when its live card lists `mentions/v1`. Read at every
        // send, so a retry names what the registry says at the retry.
        let mentions = self.app.mention_infos(&mentions).await;
        // The card is read again, now, for a message that names an access tier or keeps a cap
        // (ADR 0061): it goes out with the tier only if the agent still honours it, or with the
        // highest tier its card offers within the cap. A card that stopped honouring the tier, has
        // none within the cap, or lists the extension badly fails the job and nothing is sent; one
        // that cannot be read is retried. A message is never sent without its tier, and never under
        // the agent's default instead; an agent whose card still lists no tier gets the plain
        // message (a cap cannot be said to it).
        let sent_tier = match self.app.tier_for_send(&ctx.endpoint, tier, tier_cap).await {
            Ok(sent_tier) => sent_tier,
            Err(e) => return self.send_failed(&row, e).await,
        };
        let req = SendRequest {
            endpoint: ctx.endpoint.clone(),
            message_id: row.id.to_string(),
            context_id: binding.context_id.clone(),
            task_id: continues.clone(),
            reference_task_ids,
            content,
            release,
            ui_catalog,
            // Who the agent is to be given the thread's tools as, and which servers are attached
            // that it may use (ids, names and descriptions: no URL, no credential). The adapter
            // turns it into a token when it sends, if the card lists the extension; nothing of
            // it is stored. The servers are read now, not when the person attached them, so a
            // retry sends what is attached at the retry.
            thread_tools: Some(
                ToolsGrant::main(row.thread_id, job, ctx.agent.clone())
                    .with_attached(self.app.attached_for(&ctx.agent, &tools))
                    // what the deployment offers for large files (uploads/v1, ADR 0058), read now:
                    // the adapter tells the agent only if its live card lists the extension
                    .with_uploads(self.app.upload_offer()),
            ),
            history,
            steer: false,
            mentions,
            tier: sent_tier,
            tier_cap: plain_cap(sent_tier, tier_cap),
            // the language of the screen the message came from, as the row holds it (ADR 0085): a
            // hint, so nothing is read from the card here and a card without the extension gets the
            // plain message
            locale,
            pick,
        };
        // A message that starts a task of its own while the thread has meanwhile ended is adopted
        // (open question 33): `consume` tells the core, from the first event.
        let adopt = match (&row.payload, &continues) {
            (
                OutboxPayload::Delegate {
                    text,
                    mentions,
                    tier,
                    tier_cap,
                    locale,
                    pick,
                    ..
                },
                None,
            ) => Some(Adopt {
                text: text.clone(),
                mentions: mentions.clone(),
                tier: *tier,
                tier_cap: *tier_cap,
                locale: locale.clone(),
                pick: pick.clone(),
                previous: binding.task_id.clone(),
            }),
            _ => None,
        };
        // What the agent's card says it is, noted for the thread's export (ADR 0053); never a
        // reason not to send.
        self.app
            .record_agent_build(row.thread_id, &ctx.endpoint)
            .await;
        match self.app.ports().agents().send_stream(req).await {
            Ok(stream) => {
                let guard_stale = continues.is_some();
                match self
                    .consume(&ctx, stream, true, adopt.as_ref(), guard_stale)
                    .await?
                {
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

    /// Follows the round of a child thread's session (`Command::Observe`, ADR 0072) without sending
    /// anything: the job of the thread is the round's (`RoundStarted`, once, under the claim of
    /// this row), the row records the round's task as the one it reached, and the task is followed
    /// like any other: resubscribe, else poll. The agent is never sent a message for it. When the
    /// round ends, the parent hears of it before the row ends.
    ///
    /// A claim that lapsed after the round started finds the row sent and follows the task again.
    /// A thread that works on another round (an earlier one still going) is tried again later, and
    /// so is a round past [`MAX_OBSERVED_PER_PARENT`]. A row that can never be followed (another
    /// session's, round 0) is dead-lettered at once; one whose thread was stopped, or whose task
    /// the thread already holds as over, is skipped. A task of another context than the child's is
    /// refused, counted, and fails the follow (fail closed).
    async fn observe(&self, row: OutboxItem) -> Done {
        let OutboxPayload::Observe {
            session,
            round,
            task_id,
            by,
        } = row.payload.clone()
        else {
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
            mut ctx,
            state,
            binding,
            ..
        }) = self.load(&row).await?
        else {
            return Ok(());
        };
        let Some(child) = self.store().get_thread(None, row.thread_id).await? else {
            return self.finish(&row, OutboxFinal::Skipped).await;
        };
        // a session that is no child's of this thread, or round 0, can never be followed
        if round == 0
            || child
                .job
                .child
                .as_ref()
                .is_none_or(|c| c.session != session)
        {
            return self
                .finish(
                    &row,
                    OutboxFinal::Dead {
                        error: format!("the thread is not the child of session {session}"),
                    },
                )
                .await;
        }
        // held for as long as the round is followed: one parent's rounds take at most
        // `MAX_OBSERVED_PER_PARENT` of this process's workers
        let _slot = match child.child_of.as_ref().map(|of| of.thread) {
            Some(parent) => match self.observing_slot(parent) {
                Some(slot) => Some(slot),
                None => {
                    return self
                        .retry(
                            &row,
                            self.backoff(row.attempts),
                            format!("{MAX_OBSERVED_PER_PARENT} rounds of its parent are followed already"),
                        )
                        .await;
                }
            },
            None => None,
        };
        if row.sent_at.is_none() {
            // stopped before it began, or a task the thread has seen end: nothing to follow
            let over = binding.task_id.as_deref() == Some(task_id.as_str())
                && binding.task_state.is_some_and(AgentTaskState::is_terminal);
            // a round this thread sent itself (its message's, or the close's) is no round to
            // follow: its job, if any, follows it already
            let ledger = child.job.child.as_ref();
            let sent_itself = ledger.is_some_and(|c| {
                (c.started >= round && child.job.observed.as_ref().map(|o| o.number) != Some(round))
                    || (c.close_asked && c.track.close_round == round)
            });
            if state == ThreadState::Cancelled || over || sent_itself {
                return self.finish(&row, OutboxFinal::Skipped).await;
            }
            let lease = self.lease(&row);
            let input = Input::RoundStarted {
                session,
                number: round,
                task_id: task_id.clone(),
                by,
            };
            match self
                .app
                .apply(
                    row.thread_id,
                    input,
                    Some(format!("observe:{}", row.id)),
                    None,
                    Some(&lease),
                )
                .await
            {
                Ok(ApplyOutcome::Fenced) => return Err(DispatchError::Fenced),
                // written before, and its row not marked: mark it now
                Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => {}
                Err(AppError::Transition(TransitionError::InvalidInState { state, .. })) => {
                    if row.attempts >= self.cfg.max_attempts {
                        return self
                            .finish(
                                &row,
                                OutboxFinal::Dead {
                                    error: format!(
                                        "the child thread was {state:?} and could not follow round {round}"
                                    ),
                                },
                            )
                            .await;
                    }
                    return self
                        .retry(
                            &row,
                            self.backoff(row.attempts),
                            format!("the child thread is {state:?}"),
                        )
                        .await;
                }
                Err(AppError::NotFound) => return self.finish(&row, OutboxFinal::Skipped).await,
                Err(e) => return Err(e.into()),
            }
            if !self
                .store()
                .mark_sent(
                    &ctx.lease,
                    sent_binding(task_id.clone(), None, None, None),
                    self.now(),
                )
                .await?
            {
                return Ok(());
            }
            // the job is the round's now: what the follow names (its uploads) is read after the start
            if let Some(started) = self.store().get_thread(None, row.thread_id).await? {
                ctx.job = started.job.number;
            }
        }
        match self.follow_task(&ctx, task_id).await {
            Err(DispatchError::ForeignContext) => {
                let why = "the agent answered the round with a task of another context".to_owned();
                self.give_up(&row, why.clone(), why, false).await
            }
            other => other,
        }
    }

    /// A place among the rounds of `parent` this process follows, or `None` when
    /// [`MAX_OBSERVED_PER_PARENT`] are followed.
    fn observing_slot(&self, parent: ThreadId) -> Option<ObservingSlot<'_>> {
        let mut map = self
            .observing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = map.entry(parent).or_insert(0);
        if *n >= MAX_OBSERVED_PER_PARENT {
            return None;
        }
        *n += 1;
        Some(ObservingSlot {
            map: &self.observing,
            parent,
        })
    }

    /// The task of this row ended its turn: a round of a child session tells the parent before the
    /// row ends, and then the row is delivered.
    async fn delivered(&self, ctx: &Ctx) -> Done {
        if matches!(ctx.row.payload, OutboxPayload::Observe { .. }) {
            self.app.report_round_over(ctx.thread).await?;
        }
        self.finish(&ctx.row, OutboxFinal::Delivered).await
    }

    /// The agent made a task of its own for the message of this delegation. If the thread ended
    /// meanwhile (`done`, `failed`, or `verifying` the first task), the message is applied as a
    /// redelivery that was sent: the next job starts, and the dispatcher goes on with this task.
    /// `false` if the lease is gone.
    async fn adopt(&self, ctx: &Ctx, adopt: &Adopt) -> Result<bool, DispatchError> {
        let state = self
            .store()
            .get_thread(None, ctx.thread)
            .await?
            .map(|thread| thread.state);
        if !matches!(
            state,
            Some(ThreadState::Done | ThreadState::Failed | ThreadState::Verifying)
        ) {
            return Ok(true);
        }
        tracing::info!(
            id = %ctx.row.id,
            ?state,
            "the agent took the message as a new task of a thread that ended; adopting it"
        );
        let input = Input::Redeliver {
            text: adopt.text.clone(),
            sent: true,
            mentions: adopt.mentions.clone(),
            tier: adopt.tier,
            tier_cap: adopt.tier_cap,
            locale: adopt.locale.clone(),
            pick: adopt.pick.clone(),
        };
        match self
            .app
            .apply(
                ctx.thread,
                input,
                Some(format!("adopt:{}", ctx.row.id)),
                None,
                Some(&ctx.lease),
            )
            .await
        {
            Ok(ApplyOutcome::Fenced) => Ok(false),
            Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => Ok(true),
            // the thread moved on again (a stop): the task's updates are as late as they were
            Err(AppError::Transition(TransitionError::InvalidInState { .. })) => Ok(true),
            Err(e) => Err(e.into()),
        }
    }

    // ------------------------------------------------------------------- steer

    /// Sends a message the person wrote while the job runs into the agent's **running task**
    /// (`steer/v1`, ADR 0036). It is delivered only if the thread is `working`, the binding's task is
    /// `submitted` or `working`, the agent's live card (read by the adapter for this very send)
    /// lists the extension, and the agent answers with the same task, not ended. Anything else is
    /// not a loss: the row becomes the delegation it stands for, behind the one in flight, and the
    /// message reaches the agent after the turn, as before (and is redelivered by ADR 0020's rule
    /// if the job has ended by then). An agent that cannot be reached is retried first.
    async fn steer(&self, row: OutboxItem) -> Done {
        let OutboxPayload::Steer {
            text,
            mentions,
            tier,
            tier_cap,
            locale,
            pick,
            ..
        } = row.payload.clone()
        else {
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
            job,
            tools,
            binding,
            is_child: _,
            forked_at: _,
        }) = self.load(&row).await?
        else {
            return Ok(());
        };
        let running = binding.task_id.clone().filter(|_| {
            state == ThreadState::Working
                && binding.task_state.is_some_and(|s| {
                    matches!(s, AgentTaskState::Submitted | AgentTaskState::Working)
                })
        });
        let Some(task_id) = running else {
            return self.steer_falls_back(&row, "no task is running").await;
        };
        // The agents the message mentions, named as the registry gives them now (ADR 0026), as for
        // a delegation: the adapter tells the agent only when its live card lists `mentions/v1`.
        let mentions = self.app.mention_infos(&mentions).await;
        // The tier it goes under is decided by the live card now, as for a delegation. A steer the
        // card cannot give one to (the tier is no longer honoured, nothing within the cap, a
        // listing that cannot be read) is not sent into the task: it becomes the delegation, which
        // fails with the reason. A card that cannot be read is retried first.
        let sent_tier = match self.app.tier_for_send(&ctx.endpoint, tier, tier_cap).await {
            Ok(sent_tier) => sent_tier,
            Err(e) if e.is_retryable() && row.attempts < self.cfg.max_attempts => {
                tracing::warn!(id = %row.id, attempt = row.attempts, error = %report(&e), "steer failed; will retry");
                return self
                    .retry(&row, self.delay(row.attempts, &e), report(&e))
                    .await;
            }
            Err(e) => {
                tracing::info!(id = %row.id, error = %report(&e), "the tier of the steer cannot be decided");
                return self
                    .steer_falls_back(&row, "the agent cannot be given its access tier")
                    .await;
            }
        };
        let req = SendRequest {
            endpoint: ctx.endpoint.clone(),
            // the row's id is the message's: a row retried after a lost lease is the same message,
            // and the agent reads one `messageId` once
            message_id: row.id.to_string(),
            context_id: binding.context_id.clone(),
            task_id: Some(task_id.clone()),
            reference_task_ids: Vec::new(),
            content: SendContent::Text(text),
            release: None,
            ui_catalog: None,
            thread_tools: Some(
                ToolsGrant::main(row.thread_id, job, ctx.agent.clone())
                    .with_attached(self.app.attached_for(&ctx.agent, &tools))
                    // what the deployment offers for large files (uploads/v1, ADR 0058), read now:
                    // the adapter tells the agent only if its live card lists the extension
                    .with_uploads(self.app.upload_offer()),
            ),
            history: None,
            steer: true,
            mentions,
            tier: sent_tier,
            tier_cap: plain_cap(sent_tier, tier_cap),
            // the language of the screen the message came from, as the row holds it (ADR 0085)
            locale,
            pick,
        };
        let answer = tokio::time::timeout(STEER_ANSWER_TIMEOUT, async {
            let mut stream = self.app.ports().agents().send_stream(req).await?;
            // The agent's first event is the task (steer/v1 section 3), and the adapter says what
            // that one event holds as several envelopes: a parent task's `sessions` object
            // (child-sessions/v1, which a steer activates when the card offers it, ADR 0072) comes
            // before its status, and says no state. What names the task and says no state is
            // read past; the first envelope that says a state, or names another task, is the answer.
            while let Some(next) = stream.next().await {
                let env = next?;
                if env.task_id != task_id || env_state(&env).is_some() {
                    return Ok(Some(env));
                }
            }
            Ok(None)
        })
        .await
        .unwrap_or_else(|_| {
            Err(AgentError::unreachable(
                "the agent did not answer the steer",
            ))
        });
        match answer {
            Ok(Some(env))
                if env.task_id == task_id && env_state(&env).is_some_and(|s| !s.is_terminal()) =>
            {
                // the task reports on the stream of its own delegation; nothing more is read here
                self.finish(&row, OutboxFinal::Delivered).await
            }
            Ok(Some(env)) => {
                let why = if env.task_id == task_id {
                    "the task has ended"
                } else {
                    "the agent answered with another task"
                };
                self.steer_falls_back(&row, why).await
            }
            Ok(None) => {
                self.steer_falls_back(&row, "the agent answered with nothing")
                    .await
            }
            Err(e) if e.is_retryable() && row.attempts < self.cfg.max_attempts => {
                tracing::warn!(id = %row.id, attempt = row.attempts, error = %report(&e), "steer failed; will retry");
                self.retry(&row, self.delay(row.attempts, &e), report(&e))
                    .await
            }
            Err(e) => {
                tracing::info!(id = %row.id, error = %report(&e), "the agent did not take the steer");
                self.steer_falls_back(&row, "the agent refused it").await
            }
        }
    }

    /// The agent did not take the steer: the row becomes the delegation it stands for, which waits
    /// behind the one in flight and goes out after the turn.
    async fn steer_falls_back(&self, row: &OutboxItem, why: &str) -> Done {
        tracing::info!(id = %row.id, why, "the steer becomes a delegation, sent after the turn");
        if !self
            .store()
            .requeue_as_delegate(&self.lease(row), self.now())
            .await?
        {
            tracing::warn!(id = %row.id, "row was no longer leased to us when requeueing the steer");
        }
        Ok(())
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
            Ok(stream) => match self.consume(ctx, stream, false, None, false).await? {
                Flow::Reached => return self.delivered(ctx).await,
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
                        return self.delivered(ctx).await;
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
    /// `adopt`: the message of a delegation that opens a task of its own, and the task the thread
    /// had. When the first envelope names another task and the thread has meanwhile ended (the
    /// message was sent at the instant the first task completed, or while its verification ran), the
    /// message is applied as a redelivery that was **sent**, so the next job starts without a second
    /// delegation and this task's updates are kept instead of dropped as late (open question 33,
    /// ADR 0036).
    ///
    /// `guard_stale`: when continuing an `input-required` task, an `input-required` envelope
    /// seen before the task moved to `working` is a stale snapshot, not the answer.
    ///
    /// The pieces of a reply the agent is still writing (`text-stream/v1`) are **relayed, never
    /// applied** (ADR 0027): they go out on the wakeup port, and the whole text reaches the log
    /// once, as the agent message the agent states it as.
    async fn consume(
        &self,
        ctx: &Ctx,
        mut stream: AgentStream,
        mark_first: bool,
        adopt: Option<&Adopt>,
        guard_stale: bool,
    ) -> Result<Flow, DispatchError> {
        let mut marked = !mark_first;
        let mut seen_working = !guard_stale;
        let wakeup = self.app.ports().wakeup();
        let mut relay = LiveRelay::new(
            wakeup,
            wakeup.capabilities(),
            ctx.thread,
            ctx.agent.clone(),
            LiveTiming {
                flush: self.cfg.live_flush,
                refresh: self.cfg.live_refresh,
                max_bytes: self.cfg.live_max_bytes,
            },
        );
        loop {
            // Pieces that are due go out between envelopes; the wait for the next one is what a
            // timer may cut short.
            let due = relay.deadline();
            let next = tokio::select! {
                next = stream.next() => next,
                () = async {
                    match due {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => {
                    relay.tick().await;
                    continue;
                }
            };
            match next {
                Some(Ok(env)) => {
                    if !marked {
                        if let Some(adopt) = adopt
                            && adopt.previous.as_deref() != Some(env.task_id.as_str())
                            && !self.adopt(ctx, adopt).await?
                        {
                            return Ok(Flow::Lost);
                        }
                        let update = sent_binding(
                            env.task_id.clone(),
                            Some(env.context_id.clone()),
                            env_state(&env),
                            env.revision.clone(),
                        );
                        // The run is bound here, before the message is recorded as sent and before
                        // any of its events is applied (ADR 0084, the ruling of 2026-11-23).
                        if !env.task_id.is_empty() {
                            self.bind_run(ctx, &env.task_id).await;
                        }
                        if !self
                            .store()
                            .mark_sent(&ctx.lease, update, self.now())
                            .await?
                        {
                            return Ok(Flow::Lost);
                        }
                        marked = true;
                    }
                    if let Some(chunk) = env.live {
                        relay.chunk(chunk).await;
                        continue;
                    }
                    let state = env_state(&env);
                    if state.is_some_and(|s| !s.is_interrupted()) {
                        seen_working = true;
                    }
                    if !seen_working && state.is_some_and(AgentTaskState::is_interrupted) {
                        continue;
                    }
                    Box::pin(self.apply_envelope(ctx, &env)).await?;
                    // The whole text of a reply is in the log: no more of it is relayed.
                    if let Some(
                        AgentUpdate::Message { message_id, .. }
                        | AgentUpdate::Reasoning { message_id, .. },
                    ) = &env.update
                    {
                        relay.persisted(message_id);
                    }
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
        if let Some(expected) = &ctx.expected_context
            && &env.context_id != expected
        {
            self.app
                .child_session_dropped(ChildSessionDrop::ForeignContext, ctx.thread);
            return Err(DispatchError::ForeignContext);
        }
        // A usage report that broke the contract (ADR 0056) is never applied: it is counted, and
        // the turn goes on.
        if let Some(AgentUpdate::UsageRejected(why)) = &env.update {
            self.app
                .usage_dropped(UsageDrop::Invalid, ctx.thread, why.as_str());
            return Ok(());
        }
        // An access tier echo that is not one of the four words (ADR 0061) is never applied: it is
        // counted, and the turn goes on.
        if matches!(env.update, Some(AgentUpdate::TierRejected)) {
            self.app.tier_echo_dropped(ctx.thread);
            return Ok(());
        }
        // What an agent says of its sessions (`child-sessions/v1`, ADR 0072) is the application's:
        // a thread for each session and the parent's events. A report the adapter refused is
        // counted, and the task goes on.
        match &env.update {
            Some(AgentUpdate::ChildSessionsRejected(_)) => {
                self.app
                    .child_session_dropped(ChildSessionDrop::Rejected, ctx.thread);
                return Ok(());
            }
            Some(AgentUpdate::ChildSessions(news)) => {
                // The answer to a message this helper thread sent names its round: the round is
                // followed already (it is this job's), never observed (ADR 0072, S3).
                if let SessionNews::OfRound(report) = news
                    && ctx.expected_context.is_some()
                    && !matches!(ctx.row.payload, OutboxPayload::Observe { .. })
                    && let Some(round) = &report.round
                    && let Some(task_id) = round.task_id.clone().filter(|t| *t == env.task_id)
                {
                    let sent = Input::RoundSent {
                        session: report.session.clone(),
                        number: round.number,
                        task_id,
                    };
                    match Box::pin(self.app.apply(
                        ctx.thread,
                        sent,
                        Some(format!("roundsent:{}:{}", ctx.row.id, round.number)),
                        None,
                        Some(&ctx.lease),
                    ))
                    .await
                    {
                        Ok(ApplyOutcome::Fenced) => return Err(DispatchError::Fenced),
                        Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => {}
                        // a thread that is no child (`NotAChild` is the coded `not_child_thread`),
                        // or gone: the news says what it says
                        Err(
                            AppError::Transition(_)
                            | AppError::NotFound
                            | AppError::Arrangement {
                                code: "not_child_thread",
                                ..
                            },
                        ) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                return if Box::pin(self.app.child_sessions_news(
                    ctx.thread,
                    Some(&ctx.lease),
                    news.clone(),
                ))
                .await?
                {
                    Ok(())
                } else {
                    Err(DispatchError::Fenced)
                };
            }
            _ => {}
        }
        let key = match &env.key {
            IdemKey::Task(k) => k.clone(),
            IdemKey::Turn(k) => format!("turn:{}:{k}", ctx.row.id),
        };
        let binding = BindingUpdate {
            // adopted once by the store: the context the agent assigned (ADR 0055)
            context_id: Some(env.context_id.clone()).filter(|c| !c.is_empty()),
            task_id: Some(env.task_id.clone()),
            task_state: env_state(env),
            revision: env.revision.clone(),
        };
        // The end of a task that the binding records as over already is the same end seen twice:
        // by the stream of the delegation and by a cancel that read the task back (ADR 0036). It
        // is dropped, because the thread may have moved on to the next job meanwhile (a stop
        // that landed), and the late word of the task it abandoned must not end that job.
        if let Some(AgentUpdate::Status { state, .. }) = &env.update
            && state.is_terminal()
            && self
                .store()
                .get_binding(ctx.thread)
                .await?
                .is_some_and(|b| {
                    b.task_id.as_deref() == Some(env.task_id.as_str())
                        && b.task_state.is_some_and(AgentTaskState::is_terminal)
                })
        {
            return Ok(());
        }
        match &env.update {
            Some(update) => {
                // A file is put in the artifact store before anything is committed (ADR 0032): the
                // core is given the reference, never the bytes.
                let update = match self.ingest(ctx, update).await {
                    Some(ingested) => ingested,
                    None => update.clone(),
                };
                let call = matches!(update, AgentUpdate::Usage(UsageUpdate::Call(_)));
                // the totals of a task whose sandbox the adapter refused are applied without it
                let sandbox_rejected = match &update {
                    AgentUpdate::Usage(usage) => sandbox_rejection(usage),
                    _ => None,
                };
                let input = Input::Agent {
                    agent: ctx.agent.clone(),
                    revision: env.revision.clone().or_else(|| ctx.revision.clone()),
                    update,
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
                    // a valid call report the core did not log: the job's bound (ADR 0056)
                    Ok(ApplyOutcome::Applied { events, .. }) => {
                        if call && events.is_empty() {
                            self.app.usage_dropped(
                                UsageDrop::JobLimit,
                                ctx.thread,
                                "past the job's bound",
                            );
                        }
                        // counted once the totals are logged, so a replay (a duplicate) counts nothing
                        if let Some(why) = sandbox_rejected
                            && !events.is_empty()
                        {
                            self.app.usage_dropped(
                                UsageDrop::SandboxInvalid,
                                ctx.thread,
                                why.as_str(),
                            );
                        }
                        Ok(())
                    }
                    Ok(ApplyOutcome::Duplicate) => Ok(()),
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
                    tier: None,
                }),
                live: None,
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
            job,
            tools: _,
            binding,
            is_child: _,
            forked_at: _,
        }) = self.load(&row).await?
        else {
            return Ok(());
        };
        // A stop typed during an earlier job must not stop this one (ADR 0020). A row written
        // before it named its job means the current one.
        if let OutboxPayload::Cancel { job: Some(asked) } = &row.payload
            && *asked != job
        {
            return self.finish(&row, OutboxFinal::Skipped).await;
        }
        if state.is_terminal() {
            return self.finish(&row, OutboxFinal::Delivered).await;
        }

        // The binding keeps the last task for `referenceTaskIds` after it ended: a stop typed
        // while the next job has not reached the agent yet is not for that task (the agent would
        // refuse it, and the job would go on). Only a task that is still running is cancelled;
        // otherwise what has not been sent is.
        let running = binding
            .task_id
            .filter(|_| !binding.task_state.is_some_and(AgentTaskState::is_terminal));
        if let Some(task_id) = running {
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
                    self.reject_cancel(&row, &ctx.agent, reason, false, OutboxFinal::Delivered)
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
                        &ctx.agent,
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
            // a round of a child session that has not begun is no delegation in flight: it is
            // skipped when it is claimed (the thread is stopped), and nothing was sent for it
            .filter(|r| {
                !(matches!(r.payload, OutboxPayload::Observe { .. }) && r.sent_at.is_none())
            })
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
            &ctx.agent,
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
        agent: &AgentId,
        reason: String,
        retryable: bool,
        outcome: OutboxFinal,
    ) -> Done {
        self.apply_quiet(
            row,
            Input::CancelRejected {
                agent: agent.clone(),
                reason,
                retryable,
            },
            format!("cancelrej:{}", row.id),
        )
        .await?;
        self.finish(row, outcome).await
    }
}
