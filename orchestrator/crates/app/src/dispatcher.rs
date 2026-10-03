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
    AgentId, AgentTaskState, AgentUpdate, Classify, Event, ForkHistory, Input, ThreadId,
    ThreadState, ToolsGrant, TransitionError, fork_history, report,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, BindingUpdate, Clock,
    IdemKey, Lease, OutboxFinal, OutboxItem, OutboxKind, OutboxPayload, Ports, SendContent,
    SendRequest, StoreError, TaskHandle, TaskSnapshot, ThreadStore, Topic, Wakeup,
};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::{App, AppError, ApplyOutcome};

mod description;
mod files;
mod live;
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
    /// How often a verification in flight looks at its thread to see that it is still wanted
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
    /// The number of the thread's current job (ADR 0020).
    job: u32,
    /// The ids of the MCP servers attached to the thread when it was read (ADR 0024).
    tools: Vec<String>,
    binding: orch_ports::AgentBinding,
    /// The last event the thread copied from its parent, when it is a fork (ADR 0029): the
    /// events `1..=cut` are the conversation its first task is told.
    forked_at: Option<i64>,
}

/// A delegation that opens a task of its own: what [`Dispatcher::consume`] needs to tell, from the
/// first envelope, that the agent made a second task of a thread that has ended (open question 33).
struct Adopt {
    /// The words of the message.
    text: String,
    /// The task the binding had when the message was sent, if any.
    previous: Option<String>,
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
    /// What the delegation has kept of the agent's files so far (ADR 0032).
    files: files::Budget,
}

fn env_state(env: &AgentEnvelope) -> Option<AgentTaskState> {
    env.task_state.or(match &env.update {
        Some(AgentUpdate::Status { state, .. }) => Some(*state),
        Some(
            AgentUpdate::Artifact { .. }
            | AgentUpdate::Message { .. }
            | AgentUpdate::Ui { .. }
            | AgentUpdate::UiRejected { .. }
            | AgentUpdate::File { .. }
            | AgentUpdate::FileKept { .. }
            | AgentUpdate::FileRefused { .. }
            | AgentUpdate::Step(_),
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
fn sent_binding(
    task_id: String,
    state: Option<AgentTaskState>,
    revision: Option<String>,
) -> BindingUpdate {
    BindingUpdate {
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
                    Some(Topic::Thread(_) | Topic::Inbox) => continue,
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
            OutboxKind::Steer => self.steer(row).await,
            OutboxKind::Cancel => self.cancel(row).await,
            OutboxKind::Verify => self.verify(row).await,
            OutboxKind::Title => self.title(row).await,
            OutboxKind::Description => self.description(row).await,
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
        // The registry is read now, for every row: an agent the platform removed is dead-lettered
        // when the registry answers without it (ADR 0022), and a registry that cannot answer
        // leaves the row to be tried again, never dead.
        let entry = match self.app.resolve_agent(&binding.agent_id).await {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                self.apply_quiet(
                    row,
                    Input::DeliveryFailed {
                        reason: format!("agent '{}' is no longer listed", binding.agent_id),
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
            agent: binding.agent_id.clone(),
            endpoint: entry.endpoint.clone(),
            revision: binding.revision.clone(),
            files: files::Budget::default(),
        };
        Ok(Some(Loaded {
            ctx,
            state: thread.state,
            job: thread.job.number,
            tools: thread.job.tools,
            binding,
            forked_at: thread.forked_from.map(|origin| origin.seq),
        }))
    }

    /// The conversation a fork continues, as its first task is told it: the fork's own events
    /// `1..=cut` (the copy of its parent's), read in pages, through [`fork_history`] (ADR 0029).
    async fn fork_history(&self, thread: ThreadId, cut: i64) -> Result<ForkHistory, DispatchError> {
        let mut events: Vec<Event> = Vec::new();
        let mut after = 0;
        while after < cut {
            let page = self.store().list_events(thread, after, FORK_PAGE).await?;
            let Some(last) = page.last() else { break };
            after = last.seq;
            events.extend(page.into_iter().filter(|e| e.seq <= cut));
        }
        Ok(fork_history(&events))
    }

    async fn delegate(&self, row: OutboxItem) -> Done {
        let (content, release, new_job, ui_catalog) = match row.payload.clone() {
            OutboxPayload::Delegate {
                text,
                release,
                new_job,
                ui_catalog,
            } => (SendContent::Text(text), release, new_job, ui_catalog),
            OutboxPayload::Action {
                action,
                at,
                release,
                ui_catalog,
            } => (
                SendContent::UiAction { action, at },
                release,
                false,
                ui_catalog,
            ),
            OutboxPayload::Steer { .. }
            | OutboxPayload::Cancel { .. }
            | OutboxPayload::Verify { .. }
            | OutboxPayload::Title { .. }
            | OutboxPayload::Description { .. } => {
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
            if let OutboxPayload::Delegate { text, .. } = &row.payload
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
                .find_task_by_message(&ctx.endpoint, &binding.context_id, &row.id.to_string())
                .await
        {
            let update = sent_binding(task_id.clone(), None, None);
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
        let reference_task_ids = match (&continues, &binding.task_id) {
            (None, Some(previous)) => vec![previous.clone()],
            (Some(_) | None, _) => Vec::new(),
        };
        // The first task of a fork has no task to continue (a fork is a context of its own), so
        // it is told the conversation it continues, which is derived from the thread's own log
        // each time the message is sent: a retry sends the same words, and nothing of it is
        // stored. A task that follows another, and an action, are told nothing of it.
        let history = match (forked_at, &binding.task_id, &content) {
            (Some(cut), None, SendContent::Text(_)) => {
                Some(self.fork_history(row.thread_id, cut).await?)
            }
            _ => None,
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
                    .with_attached(self.app.attached_for(&ctx.agent, &tools)),
            ),
            history,
            steer: false,
        };
        // A message that starts a task of its own while the thread has meanwhile ended is adopted
        // (open question 33): `consume` tells the core, from the first event.
        let adopt = match (&row.payload, &continues) {
            (OutboxPayload::Delegate { text, .. }, None) => Some(Adopt {
                text: text.clone(),
                previous: binding.task_id.clone(),
            }),
            _ => None,
        };
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
        let OutboxPayload::Steer { text, .. } = row.payload.clone() else {
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
                    .with_attached(self.app.attached_for(&ctx.agent, &tools)),
            ),
            history: None,
            steer: true,
        };
        let answer = tokio::time::timeout(STEER_ANSWER_TIMEOUT, async {
            let mut stream = self.app.ports().agents().send_stream(req).await?;
            match stream.next().await {
                Some(first) => first.map(Some),
                None => Ok(None),
            }
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
                            env_state(&env),
                            env.revision.clone(),
                        );
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
                    self.apply_envelope(ctx, &env).await?;
                    // The whole text of a reply is in the log: no more of it is relayed.
                    if let Some(AgentUpdate::Message { message_id, .. }) = &env.update {
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
        let key = match &env.key {
            IdemKey::Task(k) => k.clone(),
            IdemKey::Turn(k) => format!("turn:{}:{k}", ctx.row.id),
        };
        let binding = BindingUpdate {
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
