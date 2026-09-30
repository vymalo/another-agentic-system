use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_core::{
    AgentId, AgentInfo, AgentTarget, Classify, Command, Event, EventKind, GatePolicy, Input, Job,
    Origin, Snapshot, ThreadId, ThreadRecord, ThreadState, Timestamp, UserId, WatchKey,
    is_commit_hash, repo_key, report, transition,
};
pub use orch_ports::Received;
use orch_ports::{
    AgentBinding, AgentCardInfo, AgentClient, AgentError, AgentTransport, BindingUpdate, Clock,
    Commit, CommitOutcome, IdGen, InboxFinal, InboxId, InboxLease, InboxPayload, Lease, NewEvent,
    NewInbox, NewOutbox, NewThreadRecord, NewTimer, OutboxPayload, OutboxStats, Ports, StoreError,
    TIMER_SOURCE, ThreadStore, Topic, Wakeup,
};
use tokio::time::Instant;

use crate::{AgentDirectory, AgentEntry, AppError, GateError, GateLayer, GateRules, Layer};

/// Most events an export reads unless [`AppConfig::max_export_events`] says otherwise; a longer
/// log is exported up to here and says so.
pub const DEFAULT_MAX_EXPORT_EVENTS: usize = 50_000;
/// Most bytes of serialized events (compact JSON) an export reads unless
/// [`AppConfig::max_export_bytes`] says otherwise. The event count alone does not bound memory:
/// an event may carry up to 100 000 characters of text.
pub const DEFAULT_MAX_EXPORT_BYTES: usize = 32 * 1024 * 1024;
/// Events read from the store per page when exporting.
const EXPORT_PAGE: u32 = 500;

/// The bytes `value` takes as compact JSON, counted without building the text.
fn serialized_len(value: &impl serde::Serialize) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    // An event serialises without failing (strings, numbers and enums); were it to, the part
    // counted so far is what there is.
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}
const MAX_TEXT_CHARS: usize = 100_000;
const MAX_SOURCE_CHARS: usize = 64;
const MAX_INBOX_KEY_CHARS: usize = 256;
/// Most bytes of text a stored CI report may carry (its summary is the bulk of it).
const MAX_REPORT_BYTES: usize = 64 * 1024;
const MAX_TITLE_CHARS: usize = 200;
const DEFAULT_TITLE_CHARS: usize = 80;

/// Tunables of the thread service.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// How long to wait for an agent card when listing agents or validating a release.
    pub card_timeout: Duration,
    /// Safety poll of the event stream when no wakeup arrives.
    pub stream_poll: Duration,
    /// Attempts of the optimistic commit loop.
    pub max_commit_attempts: u32,
    /// Most events [`App::export_thread`] reads; a longer log is exported up to here, flagged
    /// as truncated. Bounds the time of one export, and with `max_export_bytes` its memory.
    pub max_export_events: usize,
    /// Most bytes of serialized events (compact JSON, one comma per event) an export reads; a log
    /// that would take more is exported up to the last event that fits, flagged as truncated.
    /// Either bound cuts, and either keeps the head of the log: `seq` 1 to the last event read,
    /// with no gap.
    pub max_export_bytes: usize,
    /// The verification gate a new thread starts under; it is copied into the thread's job, so
    /// changing it never affects a running job (ADR 0016). The default requires nothing: an
    /// agent finishing is enough, as before the gate existed.
    pub gate: GatePolicy,
    /// The `gate` key of each agent's `AGENTS_FILE` entry: the layer between the deployment's
    /// gate and a thread's request (ADR 0018). Validated at startup by
    /// [`GateRules::validate`]; an agent without an entry runs under `gate` as it is.
    pub target_gates: BTreeMap<AgentId, GateLayer>,
    /// What a target or a thread may ask of the gate: which sources this build honours and the
    /// most attempts it may raise the limit to.
    pub gate_rules: GateRules,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            card_timeout: Duration::from_secs(3),
            stream_poll: Duration::from_secs(5),
            max_commit_attempts: 8,
            max_export_events: DEFAULT_MAX_EXPORT_EVENTS,
            max_export_bytes: DEFAULT_MAX_EXPORT_BYTES,
            gate: GatePolicy::default(),
            target_gates: BTreeMap::new(),
            gate_rules: GateRules::default(),
        }
    }
}

/// A configured agent with what its live card says right now (see [`App::describe_agent`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDescription {
    /// Configuration key, e.g. `coder`.
    pub id: AgentId,
    /// Display name from the configuration.
    pub name: String,
    /// The live card; `None` when it could not be read in time.
    pub card: Option<AgentCardInfo>,
}

/// Everything the orchestrator holds about one thread, read for [`App::export_thread`].
///
/// It is a snapshot: `events` end at `thread.last_seq` (events appended while it was read are
/// left out), so the job ledger in `thread` and the log agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadExport {
    /// The thread, with its full job ledger.
    pub thread: ThreadRecord,
    /// The A2A side: agent, context and task; `None` for a thread with none.
    pub binding: Option<AgentBinding>,
    /// The event log in order, from `seq` 1, within [`AppConfig::max_export_events`] and
    /// [`AppConfig::max_export_bytes`].
    pub events: Vec<Event>,
    /// `true` when the log is longer than `events` (either bound cut it).
    pub truncated: bool,
    /// When the snapshot was taken, by the application clock.
    pub exported_at: Timestamp,
}

/// Contract `NewThread`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    /// Optional title (at most 200 characters).
    pub title: Option<String>,
    /// Agent and release.
    pub target: AgentTarget,
    /// The first message.
    pub text: String,
}

/// What a surface tells the log about the input it forwards: the ids the consumer gave, and the
/// idempotency key that makes a retry of the same input a no-op.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inbound {
    /// The id the surface gave the user message (an AG-UI message id).
    pub message_id: Option<String>,
    /// The id of the run the message starts or continues.
    pub run_id: Option<String>,
    /// Makes a replay of this input a no-op: the first event carries it, and a second commit
    /// with the same key is [`ApplyOutcome::Duplicate`].
    pub key: Option<String>,
    /// The gate the request asks for (AG-UI `forwardedProps["vymalo.gate"]`). It applies when
    /// the request creates the thread: a thread's gate is fixed then (ADR 0016).
    pub gate: Option<GateLayer>,
    /// The surface the input came in through, recorded on the `user_message` event (ADR 0019).
    /// The default is the chat, `agui`.
    pub origin: Origin,
}

/// Result of [`App::create_thread_as`].
// One value per request; the thread carries its job ledger, which makes `Created` large.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Creation {
    /// The thread was created, with its first events.
    Created {
        /// The new thread.
        thread: ThreadRecord,
        /// Its first events, with their `seq`.
        events: Vec<Event>,
    },
    /// The caller's own thread with this id exists already: a concurrent request created it
    /// between the caller's lookup and this call. Nothing was written; look it up again.
    Exists,
}

/// Result of [`App::apply`].
// One value per input; the thread carries its job ledger, which makes `Applied` large.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The input was applied (possibly changing nothing).
    Applied {
        /// The thread afterwards.
        thread: ThreadRecord,
        /// The events appended, with their `seq`.
        events: Vec<Event>,
    },
    /// An idempotency key was already recorded: a replay, nothing written.
    Duplicate,
    /// The [`Lease`] the input was applied under is no longer the current claim of its outbox
    /// row: another worker owns the delegation now, and nothing was written. The caller stops
    /// working on the row.
    Fenced,
}

/// The thread service: validation, the transition + commit loop, and live event streams.
pub struct App<P: Ports> {
    ports: P,
    agents: AgentDirectory,
    cfg: AppConfig,
    ready: AtomicBool,
    shutting_down: AtomicBool,
}

fn validate_text(text: &str) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Err(AppError::Invalid("text must not be empty".to_owned()));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(AppError::Invalid(format!(
            "text must be at most {MAX_TEXT_CHARS} characters"
        )));
    }
    Ok(())
}

fn default_title(text: &str) -> String {
    let first = text.trim().lines().next().unwrap_or("").trim();
    first.chars().take(DEFAULT_TITLE_CHARS).collect()
}

impl<P: Ports> App<P> {
    /// Builds the service. It starts ready; a composition root that runs migrations first
    /// may call [`App::set_ready`] to control readiness itself.
    ///
    /// The gate in `cfg` is checked here, whichever composition root built it: the deployment's
    /// policy and every agent's resolved gate must ask only for what `cfg.gate_rules` honours,
    /// with attempts in range and a verifier that is another configured agent
    /// ([`GateRules::validate`]). A gate that could never pass is an error now (the binary
    /// exits 78), not a 500 on every request later.
    pub fn new(ports: P, agents: AgentDirectory, cfg: AppConfig) -> Result<Self, GateError> {
        cfg.gate_rules
            .validate(&cfg.gate, &cfg.target_gates, &agents)?;
        Ok(App {
            ports,
            agents,
            cfg,
            ready: AtomicBool::new(true),
            shutting_down: AtomicBool::new(false),
        })
    }

    /// The ports this service runs on.
    pub fn ports(&self) -> &P {
        &self.ports
    }

    /// The configured agents.
    pub fn directory(&self) -> &AgentDirectory {
        &self.agents
    }

    /// Marks the service (not) ready for `/readyz`.
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }

    /// Marks the process as shutting down (`/healthz` answers 503).
    pub fn set_shutting_down(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    /// Whether shutdown started.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Ready flag set and the store reachable.
    pub async fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst) && self.ports.store().ping().await.is_ok()
    }

    /// The open outbox rows counted at the application clock's `now`, with that `now`: the
    /// numbers behind `/metrics` and autoscaling. The counts are global, over every replica's
    /// rows, whichever process answers.
    pub async fn outbox_stats(&self) -> Result<(Timestamp, OutboxStats), AppError> {
        let now = self.ports.clock().now();
        let stats = self.ports.store().outbox_stats(now).await?;
        Ok((now, stats))
    }

    async fn notify(&self, topic: Topic) {
        if let Err(e) = self.ports.wakeup().notify(topic).await {
            tracing::warn!(error = %report(&e), "wakeup notify failed; consumers fall back to polling");
        }
    }

    /// The live card of a configured agent; `None` when it cannot be read in time (fail closed,
    /// ADR 0008: nothing the card would have said is assumed).
    async fn live_card(&self, entry: &AgentEntry) -> Option<AgentCardInfo> {
        let card = tokio::time::timeout(
            self.cfg.card_timeout,
            self.ports.agents().read_card(&entry.endpoint),
        )
        .await;
        match card {
            Ok(Ok(card)) => Some(card),
            Ok(Err(e)) => {
                tracing::warn!(agent = %entry.endpoint.id, error = %report(&e), class = ?e.class(), "agent card unreadable");
                None
            }
            Err(_) => {
                tracing::warn!(agent = %entry.endpoint.id, "agent card timed out");
                None
            }
        }
    }

    /// Lists the configured agents with their live card data. A card that cannot be read
    /// in time yields an agent without `description` and `releases` (fail closed, ADR 0008).
    pub async fn list_agents(&self) -> Vec<AgentInfo> {
        let lookups = self.agents.iter().map(|entry| async move {
            let (description, releases) = match self.live_card(entry).await {
                Some(card) => (card.description, card.releases),
                None => (None, None),
            };
            // Only an A2A agent has a card URL; the contract's `cardUrl` is optional for that.
            let card_url = match &entry.endpoint.transport {
                AgentTransport::A2a { card_url, .. } => Some(card_url.clone()),
                AgentTransport::Local { .. } => None,
            };
            AgentInfo {
                id: entry.endpoint.id.clone(),
                name: entry.name.clone(),
                description,
                card_url,
                releases,
            }
        });
        futures::future::join_all(lookups).await
    }

    /// One configured agent and its live card, read now and never cached; `None` when no agent
    /// has this id. `card` is `None` when the card cannot be read in time (fail closed, ADR 0008).
    pub async fn describe_agent(&self, id: &AgentId) -> Option<AgentDescription> {
        let entry = self.agents.get(id)?;
        Some(AgentDescription {
            id: entry.endpoint.id.clone(),
            name: entry.name.clone(),
            card: self.live_card(entry).await,
        })
    }

    async fn validate_target(&self, target: &AgentTarget) -> Result<(), AppError> {
        let entry = self
            .agents
            .get(&target.agent_id)
            .ok_or_else(|| AppError::Invalid(format!("unknown agent '{}'", target.agent_id)))?;
        let Some(release) = &target.release else {
            return Ok(());
        };
        let card = tokio::time::timeout(
            self.cfg.card_timeout,
            self.ports.agents().read_card(&entry.endpoint),
        )
        .await;
        // Fail closed: a release cannot be validated without the live card. That is the
        // agent's failure, not the caller's mistake.
        let releases = match card {
            Ok(Ok(card)) => card.releases,
            Ok(Err(e)) => return Err(AppError::upstream(&target.agent_id, e)),
            Err(_) => {
                return Err(AppError::upstream(
                    &target.agent_id,
                    AgentError::unreachable(format!(
                        "the agent card did not arrive within {:?}",
                        self.cfg.card_timeout
                    )),
                ));
            }
        };
        let releases = releases
            .ok_or_else(|| AppError::Invalid("agent does not offer releases".to_owned()))?;
        if releases.accepts(release) {
            Ok(())
        } else {
            Err(AppError::Invalid(format!("unknown release '{release}'")))
        }
    }

    /// Creates a thread whose first message is already in the event log and queued for delegation.
    pub async fn create_thread(
        &self,
        user: &UserId,
        req: NewThread,
    ) -> Result<ThreadRecord, AppError> {
        let id = ThreadId(self.ports.ids().new_id());
        match self
            .create_thread_as(user, id, req, Inbound::default())
            .await?
        {
            Creation::Created { thread, .. } => Ok(thread),
            Creation::Exists => Err(AppError::internal(
                "a freshly minted thread id was already taken",
            )),
        }
    }

    /// Creates the thread `id`, which the caller chose (a consumer-minted id, as AG-UI has it),
    /// with `req.text` as its first message, and records `inbound` in the log.
    ///
    /// An id that belongs to someone else is [`AppError::NotFound`], the same answer as for an
    /// id that does not exist for the caller, so a collision does not reveal the other thread.
    /// The caller's own thread with that id is [`Creation::Exists`] (only a concurrent request
    /// with the same id can have created it).
    pub async fn create_thread_as(
        &self,
        user: &UserId,
        id: ThreadId,
        req: NewThread,
        inbound: Inbound,
    ) -> Result<Creation, AppError> {
        validate_text(&req.text)?;
        if let Some(title) = &req.title
            && title.chars().count() > MAX_TITLE_CHARS
        {
            return Err(AppError::Invalid(format!(
                "title must be at most {MAX_TITLE_CHARS} characters"
            )));
        }
        self.validate_target(&req.target).await?;
        let gate = self.resolve_gate(&req.target.agent_id, inbound.gate.as_ref())?;

        let now = self.ports.clock().now();
        let (next, cmds) = transition(
            &Snapshot::queued(gate),
            &Input::UserMessage {
                user: user.clone(),
                text: req.text.clone(),
                message_id: inbound.message_id,
                run_id: inbound.run_id,
                origin: inbound.origin,
            },
        )?;
        let title = req
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| default_title(&req.text));
        // The default job is what the column holds when nothing is written.
        let job = (next.job != Job::default()).then_some(next.job);
        let commit = self.build_commit(
            &req.target,
            next.state,
            job,
            cmds,
            inbound.key.as_deref(),
            None,
            now,
        );
        let new = NewThreadRecord {
            id,
            owner: user.clone(),
            title,
            target: req.target,
            context_id: id.to_string(),
            now,
        };
        match self.ports.store().create_thread(new, commit).await {
            Ok((thread, events)) => {
                self.notify(Topic::Thread(id)).await;
                self.notify(Topic::Outbox).await;
                Ok(Creation::Created { thread, events })
            }
            Err(e) => {
                // A chosen id can be taken. The store reports that as a failure like any other,
                // so look: someone's thread with this id is a collision, nothing there is a
                // real failure.
                match self.ports.store().get_thread(None, id).await {
                    Ok(Some(existing)) if &existing.owner == user => Ok(Creation::Exists),
                    Ok(Some(_)) => Err(AppError::NotFound),
                    Ok(None) | Err(_) => Err(e.into()),
                }
            }
        }
    }

    /// The gate a new thread of `agent` starts under: the deployment's, then the agent's entry,
    /// then what the request asked for. A request that asks for less than the layers above
    /// require, or for something this build cannot honour, is [`AppError::Invalid`]; a fault in
    /// the agent's own entry is not the caller's, so it is an internal error.
    pub fn resolve_gate(
        &self,
        agent: &AgentId,
        request: Option<&GateLayer>,
    ) -> Result<GatePolicy, AppError> {
        let rules = &self.cfg.gate_rules;
        let mut policy = rules
            .for_target(&self.cfg.gate, agent, self.cfg.target_gates.get(agent))
            .map_err(|e| {
                AppError::internal(format!("the gate of agent {agent} is invalid: {e}"))
            })?;
        if let Some(request) = request {
            policy = rules
                .apply(&policy, request, &Layer::Thread)
                .map_err(|e| AppError::Invalid(e.to_string()))?;
            // A thread may require the verifier but not choose it: what it asked for must have
            // one, and it must be another agent than the one being verified.
            rules
                .check_verifier(&policy, agent, &self.agents, &Layer::Thread)
                .map_err(|e| AppError::Invalid(e.to_string()))?;
        }
        Ok(policy)
    }

    /// Whether a run's gate request, put on the gate the thread already has, would change it.
    /// A thread's gate is fixed when it is created (ADR 0016), so a surface refuses a run whose
    /// request differs instead of dropping it. `Ok(false)` for a request that says what the
    /// thread has; [`AppError::Invalid`] for one the rules refuse in any case (a source removed,
    /// attempts out of range, something this build cannot honour).
    pub fn gate_request_changes(
        &self,
        current: &GatePolicy,
        request: &GateLayer,
    ) -> Result<bool, AppError> {
        let requested = self
            .cfg
            .gate_rules
            .apply(current, request, &Layer::Thread)
            .map_err(|e| AppError::Invalid(e.to_string()))?;
        Ok(&requested != current)
    }

    /// The thread `id` when it exists and is the user's; `None` when nothing has this id;
    /// [`AppError::NotFound`] when it belongs to someone else.
    ///
    /// The three-way answer is for surfaces that let the consumer choose thread ids: `None`
    /// means the id is free to create, and someone else's thread must look like any other
    /// refusal to the caller.
    pub async fn find_thread(
        &self,
        user: &UserId,
        id: ThreadId,
    ) -> Result<Option<ThreadRecord>, AppError> {
        match self.ports.store().get_thread(None, id).await? {
            Some(thread) if &thread.owner == user => Ok(Some(thread)),
            Some(_) => Err(AppError::NotFound),
            None => Ok(None),
        }
    }

    /// The user's threads, newest first.
    pub async fn list_threads(
        &self,
        user: &UserId,
        before: Option<ThreadId>,
        limit: u32,
    ) -> Result<Vec<ThreadRecord>, AppError> {
        Ok(self.ports.store().list_threads(user, before, limit).await?)
    }

    /// One of the user's threads; someone else's thread is `NotFound`.
    pub async fn get_thread(&self, user: &UserId, id: ThreadId) -> Result<ThreadRecord, AppError> {
        self.ports
            .store()
            .get_thread(Some(user), id)
            .await?
            .ok_or(AppError::NotFound)
    }

    /// A snapshot of one of the user's threads for sharing: the thread with its job ledger, its
    /// binding and its whole event log. Someone else's thread is `NotFound`, like every read.
    ///
    /// The log is read in pages through [`ThreadStore::list_events`]; nothing new is asked of the
    /// store. The read stops at [`AppConfig::max_export_events`] events or
    /// [`AppConfig::max_export_bytes`] bytes of serialized events, whichever comes first, and
    /// says so (`truncated`): what is read is always the head of the log.
    pub async fn export_thread(
        &self,
        user: &UserId,
        id: ThreadId,
    ) -> Result<ThreadExport, AppError> {
        let thread = self.get_thread(user, id).await?;
        let store = self.ports.store();
        let mut events: Vec<Event> = Vec::new();
        let mut bytes = 0_usize;
        let mut after = 0;
        'pages: while after < thread.last_seq {
            let page = store.list_events(id, after, EXPORT_PAGE).await?;
            let Some(last) = page.last() else { break };
            after = last.seq;
            for event in page {
                if event.seq > thread.last_seq {
                    break 'pages;
                }
                // The comma between two events counts too.
                let size = serialized_len(&event).saturating_add(1);
                if events.len() >= self.cfg.max_export_events
                    || bytes.saturating_add(size) > self.cfg.max_export_bytes
                {
                    break 'pages;
                }
                bytes += size;
                events.push(event);
            }
        }
        let truncated = events
            .last()
            .map_or(thread.last_seq > 0, |e| e.seq < thread.last_seq);
        let binding = store.get_binding(id).await?;
        Ok(ThreadExport {
            thread,
            binding,
            events,
            truncated,
            exported_at: self.ports.clock().now(),
        })
    }

    /// Events with `seq > after`, oldest first.
    pub async fn list_events(
        &self,
        user: &UserId,
        id: ThreadId,
        after: i64,
        limit: u32,
    ) -> Result<Vec<Event>, AppError> {
        self.get_thread(user, id).await?;
        Ok(self.ports.store().list_events(id, after, limit).await?)
    }

    /// The newest `limit` events of `kind` of one of the user's threads, newest first (one
    /// bounded read; someone else's thread is `NotFound`).
    pub async fn latest_events(
        &self,
        user: &UserId,
        id: ThreadId,
        kind: EventKind,
        limit: u32,
    ) -> Result<Vec<Event>, AppError> {
        self.get_thread(user, id).await?;
        Ok(self.ports.store().latest_events(id, kind, limit).await?)
    }

    /// Appends a user message and queues its delegation. Returns the `user_message` event.
    pub async fn post_message(
        &self,
        user: &UserId,
        id: ThreadId,
        text: String,
    ) -> Result<Event, AppError> {
        validate_text(&text)?;
        self.get_thread(user, id).await?;
        let outcome = self
            .apply(
                id,
                Input::UserMessage {
                    user: user.clone(),
                    text,
                    message_id: None,
                    run_id: None,
                    origin: Origin::default(),
                },
                None,
                None,
                None,
            )
            .await?;
        match outcome {
            ApplyOutcome::Applied { events, .. } => events
                .into_iter()
                .find(|e| e.kind() == EventKind::UserMessage)
                .ok_or_else(|| AppError::internal("the user message is missing from its commit")),
            ApplyOutcome::Duplicate => Err(AppError::internal(
                "a message without an idempotency key was reported as a duplicate",
            )),
            ApplyOutcome::Fenced => Err(AppError::internal(
                "a commit without a lease was reported as fenced",
            )),
        }
    }

    /// Applies an input a surface already translated (a user message with the ids the consumer
    /// gave it, an A2UI action, or a cancel) to one of the user's threads, under the idempotency `key`.
    ///
    /// The text of a user message is validated as for [`post_message`](Self::post_message).
    /// [`ApplyOutcome::Duplicate`] means the key was recorded already: an earlier request did
    /// this, and nothing was written.
    pub async fn submit(
        &self,
        user: &UserId,
        id: ThreadId,
        input: Input,
        key: Option<String>,
    ) -> Result<ApplyOutcome, AppError> {
        match &input {
            Input::UserMessage { text, .. } => validate_text(text)?,
            // The surface has checked that the thread has the surface; the sizes are checked
            // here as well, so no surface can store an oversized action.
            Input::UiAction { action, .. } => {
                action
                    .check()
                    .map_err(|e| AppError::Invalid(format!("invalid action: {e}")))?;
            }
            // Machine inputs (a redelivery, a CI report, the verifier's verdict, a timer) come from the
            // inbox and the dispatcher through `apply`, never from a user's request: a user
            // must not be able to forge a check result.
            Input::Redeliver { .. }
            | Input::CiReported(_)
            | Input::VerifierReported { .. }
            | Input::VerifierFailed { .. }
            | Input::TimerFired(_) => {
                return Err(AppError::Invalid(
                    "this input cannot be submitted by a user".to_owned(),
                ));
            }
            Input::Cancel { .. }
            | Input::Agent { .. }
            | Input::DeliveryFailed { .. }
            | Input::CancelledBeforeStart
            | Input::CancelRejected { .. } => {}
        }
        self.get_thread(user, id).await?;
        self.apply(id, input, key, None, None).await
    }

    /// Requests cancellation of the thread's running work. A finished thread is a no-op.
    pub async fn cancel(&self, user: &UserId, id: ThreadId) -> Result<(), AppError> {
        self.get_thread(user, id).await?;
        self.apply(id, Input::Cancel { user: user.clone() }, None, None, None)
            .await?;
        Ok(())
    }

    /// Turns commands into one store commit. The first appended event carries `key`, the
    /// following ones `key#1`, `key#2`, …
    #[allow(clippy::too_many_arguments)]
    fn build_commit(
        &self,
        target: &AgentTarget,
        new_state: ThreadState,
        job: Option<Job>,
        cmds: Vec<Command>,
        key: Option<&str>,
        binding: Option<BindingUpdate>,
        now: Timestamp,
    ) -> Commit {
        let mut events = Vec::new();
        let mut outbox = Vec::new();
        let mut watches = Vec::new();
        let mut timers = Vec::new();
        for cmd in cmds {
            match cmd {
                Command::Append(draft) => {
                    let idempotency_key = key.map(|k| match events.len() {
                        0 => k.to_owned(),
                        n => format!("{k}#{n}"),
                    });
                    events.push(NewEvent {
                        at: now,
                        actor: draft.actor,
                        body: draft.body,
                        idempotency_key,
                    });
                }
                Command::Delegate { text } => outbox.push(NewOutbox {
                    id: orch_ports::OutboxId(self.ports.ids().new_id()),
                    payload: OutboxPayload::Delegate {
                        text,
                        release: target.release.clone(),
                    },
                }),
                Command::DelegateAction { action } => outbox.push(NewOutbox {
                    id: orch_ports::OutboxId(self.ports.ids().new_id()),
                    payload: OutboxPayload::Action {
                        action,
                        at: now,
                        release: target.release.clone(),
                    },
                }),
                Command::RequestCancel { job } => outbox.push(NewOutbox {
                    id: orch_ports::OutboxId(self.ports.ids().new_id()),
                    payload: OutboxPayload::Cancel { job: Some(job) },
                }),
                // Both are written in this commit: the watch also re-arms the reports that
                // were parked waiting for it, and the timer becomes an inbox row that the
                // store makes due `after` this commit's `now` (the core never reads a clock).
                Command::Watch { key } => {
                    // The two sides of a CI match log the same key (here, and in `receive`), so a
                    // report that never finds its job can be compared by eye with what the job
                    // watches: a repository spelled differently shows at once.
                    let (repository, short_sha) = describe_watch_key(key.as_str());
                    tracing::info!(
                        watch_key = %key,
                        repository,
                        short_sha,
                        "watching for the CI reports of a pushed commit"
                    );
                    watches.push(key);
                }
                Command::Schedule { after, timer } => timers.push(NewTimer {
                    id: InboxId(self.ports.ids().new_id()),
                    after,
                    timer,
                }),
                // The request is an outbox row in this commit, so it cannot be lost or made
                // twice; the dispatcher asks the verifier and feeds the verdict back.
                Command::RequestVerification {
                    attempt,
                    verification,
                    verifier,
                    pushed,
                    text,
                } => outbox.push(NewOutbox {
                    id: orch_ports::OutboxId(self.ports.ids().new_id()),
                    payload: OutboxPayload::Verify {
                        attempt,
                        verification,
                        verifier,
                        pushed,
                        text,
                    },
                }),
            }
        }
        Commit {
            new_state,
            job,
            events,
            outbox,
            binding,
            now,
            lease: None,
            watches,
            timers,
            inbox: None,
        }
    }

    /// Applies `input` to the thread: `transition`, then one store commit, retried on version
    /// conflicts. `key` makes a replayed input idempotent; `binding` is persisted in the same
    /// transaction. Used by the API (`lease: None`) and by the dispatcher, which passes the
    /// claim of the outbox row it works for: once another worker has claimed the row, the
    /// commit is refused and the result is [`ApplyOutcome::Fenced`] (not retried).
    pub async fn apply(
        &self,
        thread: ThreadId,
        input: Input,
        key: Option<String>,
        binding: Option<BindingUpdate>,
        lease: Option<&Lease>,
    ) -> Result<ApplyOutcome, AppError> {
        self.apply_fenced(thread, input, key, binding, lease, None)
            .await
    }

    /// Applies an input the inbox delivered (a timer, a CI report) under the worker's claim on
    /// the inbox row `lease`. The idempotency key is `inbox:<row id>`. The row is marked
    /// `applied` in the same commit as the thread's change, so the two cannot come apart; when
    /// the input changes nothing (or is a repeat) the row is completed on its own. Either way
    /// [`ApplyOutcome::Applied`] or [`ApplyOutcome::Duplicate`] means the row is finished.
    /// Once another worker has claimed the row the result is [`ApplyOutcome::Fenced`] and
    /// nothing was written.
    pub async fn apply_from_inbox(
        &self,
        thread: ThreadId,
        input: Input,
        lease: &InboxLease,
    ) -> Result<ApplyOutcome, AppError> {
        let key = format!("inbox:{}", lease.id);
        self.apply_fenced(thread, input, Some(key), None, None, Some(lease))
            .await
    }

    async fn apply_fenced(
        &self,
        thread: ThreadId,
        input: Input,
        key: Option<String>,
        binding: Option<BindingUpdate>,
        lease: Option<&Lease>,
        inbox: Option<&InboxLease>,
    ) -> Result<ApplyOutcome, AppError> {
        for _ in 0..self.cfg.max_commit_attempts {
            let record = self
                .ports
                .store()
                .get_thread(None, thread)
                .await?
                .ok_or(AppError::NotFound)?;
            let (next, cmds) = transition(&record.snapshot(), &input)?;
            let now = self.ports.clock().now();
            let job = (next.job != record.job).then_some(next.job);
            let next = next.state;
            let mut commit = self.build_commit(
                &record.target,
                next,
                job,
                cmds,
                key.as_deref(),
                binding.clone(),
                now,
            );
            commit.lease = lease.cloned();
            commit.inbox = inbox.cloned();
            if commit.events.is_empty()
                && commit.outbox.is_empty()
                && commit.binding.is_none()
                && commit.job.is_none()
                && commit.watches.is_empty()
                && commit.timers.is_empty()
                && next == record.state
                // An inbox row is still owed its completion. That is a commit of its own
                // (it carries the claim and nothing else), not a bare `complete_inbox`: the
                // store checks the version too, so a "nothing to do" that was decided on a
                // thread that has moved since is decided again, not written as final.
                && inbox.is_none()
            {
                return Ok(ApplyOutcome::Applied {
                    thread: record,
                    events: Vec::new(),
                });
            }
            let has_outbox = !commit.outbox.is_empty();
            let has_watches = !commit.watches.is_empty();
            match self
                .ports
                .store()
                .commit(thread, record.version, commit)
                .await
            {
                Ok(CommitOutcome::Applied { thread: t, events }) => {
                    if !events.is_empty() || t.state != record.state {
                        self.notify(Topic::Thread(thread)).await;
                    }
                    if has_outbox {
                        self.notify(Topic::Outbox).await;
                    }
                    if has_watches {
                        // The commit may have re-armed parked reports.
                        self.notify(Topic::Inbox).await;
                    }
                    return Ok(ApplyOutcome::Applied { thread: t, events });
                }
                Ok(CommitOutcome::Duplicate) => {
                    return match inbox {
                        Some(inbox) => self.finish_inbox(inbox, ApplyOutcome::Duplicate).await,
                        None => Ok(ApplyOutcome::Duplicate),
                    };
                }
                Ok(CommitOutcome::Fenced) => return Ok(ApplyOutcome::Fenced),
                Err(StoreError::VersionConflict) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(AppError::Contended)
    }

    /// Completes the inbox row whose input was applied already (its events are in the log under
    /// the row's key). `outcome` is what to answer when the row was still ours; a lost claim is
    /// [`ApplyOutcome::Fenced`].
    async fn finish_inbox(
        &self,
        lease: &InboxLease,
        outcome: ApplyOutcome,
    ) -> Result<ApplyOutcome, AppError> {
        let done = self
            .ports
            .store()
            .complete_inbox(lease, InboxFinal::Applied, self.ports.clock().now())
            .await?;
        Ok(if done { outcome } else { ApplyOutcome::Fenced })
    }

    /// Stores an unsolicited report for the inbox worker to apply, and returns at once: the entry
    /// point of the surfaces that take input from machines (the webhooks, ADR 0017). `source`
    /// says who sent it and `idempotency_key` is the sender's id of this delivery; a repeat of
    /// the pair is [`Received::Duplicate`] and stores nothing, so a redelivery is harmless.
    ///
    /// The caller has authenticated the delivery and normalised it to a payload; a payload is
    /// data, and nothing here trusts it beyond its shape. Which thread a CI report is about is
    /// not for the sender to say: its correlation is always the watch key of its repository and
    /// commit ([`WatchKey::ci`](orch_core::WatchKey::ci)), built here from the report after both
    /// are put in the form the watches use ([`repo_key`], a lower-case full hash), so a report
    /// can reach only a thread that asked for that very commit. A report whose watch does not
    /// exist yet waits (parked) until a thread starts watching, or expires.
    ///
    /// # Errors
    /// [`AppError::Invalid`] for a `source` or key that is empty, longer than 64 or 256
    /// characters, or not printable ASCII (they reach logs and spans); for the reserved source
    /// `timer`; for a payload that a surface may not send (a timer: only the core arms those);
    /// for an oversized report; and for a report whose repository is not a repository address or
    /// whose `sha` is not a full commit hash (such a report could never match a watch, so it is
    /// refused rather than parked until it expires).
    pub async fn receive(
        &self,
        source: &str,
        idempotency_key: &str,
        payload: InboxPayload,
    ) -> Result<Received, AppError> {
        let bounded = |what: &str, value: &str, max: usize| -> Result<(), AppError> {
            if value.trim().is_empty() || value.chars().count() > max {
                return Err(AppError::Invalid(format!(
                    "{what} must be 1 to {max} characters"
                )));
            }
            if !value.chars().all(|c| (' '..='~').contains(&c)) {
                return Err(AppError::Invalid(format!("{what} must be printable ASCII")));
            }
            Ok(())
        };
        bounded("source", source, MAX_SOURCE_CHARS)?;
        bounded("idempotency key", idempotency_key, MAX_INBOX_KEY_CHARS)?;
        if source == TIMER_SOURCE {
            return Err(AppError::Invalid(format!(
                "the source {TIMER_SOURCE} is reserved for the orchestrator's own timers"
            )));
        }
        let (payload, correlation) = match payload {
            InboxPayload::Timer { .. } => {
                return Err(AppError::Invalid(
                    "timers are armed by the orchestrator and cannot be received".to_owned(),
                ));
            }
            InboxPayload::CiReport(mut report) => {
                let text = report.repository.len()
                    + report.sha.len()
                    + report.name.len()
                    + [&report.branch, &report.url, &report.summary]
                        .into_iter()
                        .flatten()
                        .map(String::len)
                        .sum::<usize>();
                if text > MAX_REPORT_BYTES {
                    return Err(AppError::Invalid(format!(
                        "a CI report may carry at most {MAX_REPORT_BYTES} bytes of text"
                    )));
                }
                // The form a watch key is built from (`transition` builds it from the pushed
                // ref, which is normalised the same way).
                let Some(repository) = repo_key(&report.repository) else {
                    return Err(AppError::Invalid(
                        "the repository of a CI report must be a repository address".to_owned(),
                    ));
                };
                let sha = report.sha.trim().to_lowercase();
                if !is_commit_hash(&sha) {
                    return Err(AppError::Invalid(
                        "the sha of a CI report must be a full commit hash".to_owned(),
                    ));
                }
                report.repository = repository;
                report.sha = sha;
                let correlation = WatchKey::ci(&report.repository, &report.sha).to_string();
                let (repository, short_sha) = describe_watch_key(&correlation);
                tracing::info!(
                    watch_key = %correlation,
                    repository,
                    short_sha,
                    name = %report.name,
                    conclusion = report.conclusion.as_str(),
                    source,
                    "a CI report will be matched to the job that watches this key"
                );
                (InboxPayload::CiReport(report), correlation)
            }
        };
        let row = NewInbox {
            id: InboxId(self.ports.ids().new_id()),
            source: source.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
            payload,
            correlation: Some(correlation),
        };
        let received = self
            .ports
            .store()
            .receive(row, self.ports.clock().now())
            .await?;
        if matches!(received, Received::Stored { .. }) {
            self.notify(Topic::Inbox).await;
        }
        Ok(received)
    }

    /// Persists binding fields (task id, state, revision) without changing the thread. `lease`
    /// as in [`apply`](Self::apply): [`ApplyOutcome::Fenced`] when it is no longer current.
    pub async fn record_binding(
        &self,
        thread: ThreadId,
        binding: BindingUpdate,
        lease: Option<&Lease>,
    ) -> Result<ApplyOutcome, AppError> {
        for _ in 0..self.cfg.max_commit_attempts {
            let record = self
                .ports
                .store()
                .get_thread(None, thread)
                .await?
                .ok_or(AppError::NotFound)?;
            let commit = Commit {
                new_state: record.state,
                job: None,
                events: Vec::new(),
                outbox: Vec::new(),
                binding: Some(binding.clone()),
                now: self.ports.clock().now(),
                lease: lease.cloned(),
                watches: Vec::new(),
                timers: Vec::new(),
                inbox: None,
            };
            match self
                .ports
                .store()
                .commit(thread, record.version, commit)
                .await
            {
                Ok(CommitOutcome::Applied { thread, events }) => {
                    return Ok(ApplyOutcome::Applied { thread, events });
                }
                Ok(CommitOutcome::Duplicate) => return Ok(ApplyOutcome::Duplicate),
                Ok(CommitOutcome::Fenced) => return Ok(ApplyOutcome::Fenced),
                Err(StoreError::VersionConflict) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(AppError::Contended)
    }

    /// Replays every event with `seq > after` (`after` beyond the end of the log counts as the
    /// end), then streams live ones, without gaps or duplicates. Wakeups make it prompt; a
    /// periodic poll makes it correct without them.
    /// Once shutdown started and the stream has caught up, it ends (within one poll interval).
    pub async fn event_stream(
        self: &Arc<Self>,
        user: &UserId,
        id: ThreadId,
        after: i64,
    ) -> Result<BoxStream<'static, Event>, AppError> {
        let thread = self.get_thread(user, id).await?;
        // A cursor beyond the end of the log names events that do not exist (a stale or forged
        // `Last-Event-ID`). Left as it is, the stream would stay silent until the log caught
        // up with it; clamped, the client gets every event that happens from now on.
        let after = after.clamp(0, thread.last_seq);
        // Subscribe before the first read so nothing between read and subscribe is lost.
        let wake = self.ports.wakeup().subscribe();
        struct St<P: Ports> {
            app: Arc<App<P>>,
            id: ThreadId,
            cursor: i64,
            buf: VecDeque<Event>,
            wake: BoxStream<'static, Topic>,
            wake_open: bool,
        }
        let st = St {
            app: Arc::clone(self),
            id,
            cursor: after,
            buf: VecDeque::new(),
            wake,
            wake_open: true,
        };
        Ok(futures::stream::unfold(st, |mut st| async move {
            loop {
                if let Some(event) = st.buf.pop_front() {
                    st.cursor = event.seq;
                    return Some((event, st));
                }
                match st
                    .app
                    .ports
                    .store()
                    .list_events(st.id, st.cursor, 500)
                    .await
                {
                    Ok(events) if !events.is_empty() => {
                        st.buf.extend(events);
                        continue;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %report(&e), "event stream read failed; retrying")
                    }
                }
                // Caught up and the process is going away: end the stream so the client
                // reconnects (with `Last-Event-ID`) to another replica and shutdown can drain.
                if st.app.is_shutting_down() {
                    return None;
                }
                let tick = tokio::time::sleep_until(Instant::now() + st.app.cfg.stream_poll);
                tokio::pin!(tick);
                loop {
                    tokio::select! {
                        topic = st.wake.next(), if st.wake_open => match topic {
                            Some(Topic::Thread(t)) if t == st.id => break,
                            Some(Topic::Resync) => break,
                            Some(Topic::Thread(_) | Topic::Outbox | Topic::Inbox) => {}
                            None => st.wake_open = false,
                        },
                        () = &mut tick => break,
                    }
                }
            }
        })
        .boxed())
    }
}

/// The repository key and the first seven characters of the commit in a CI watch key
/// (`ci:<repository>@<sha>`), for the log; the whole key when it is not of that form.
fn describe_watch_key(key: &str) -> (&str, &str) {
    let Some((repository, sha)) = key.strip_prefix("ci:").and_then(|k| k.rsplit_once('@')) else {
        return (key, "");
    };
    (repository, sha.get(..7).unwrap_or(sha))
}
