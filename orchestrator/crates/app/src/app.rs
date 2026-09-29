use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_core::{
    AgentInfo, AgentTarget, Command, Event, EventKind, Input, ThreadId, ThreadRecord, ThreadState,
    Timestamp, UserId, transition,
};
use orch_ports::{
    AgentClient, BindingUpdate, Clock, Commit, CommitOutcome, IdGen, NewEvent, NewOutbox,
    NewThreadRecord, OutboxPayload, Ports, StoreError, ThreadStore, Topic, Wakeup,
};
use tokio::time::Instant;

use crate::{AgentDirectory, AppError};

const MAX_TEXT_CHARS: usize = 100_000;
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
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            card_timeout: Duration::from_secs(3),
            stream_poll: Duration::from_secs(5),
            max_commit_attempts: 8,
        }
    }
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

/// Result of [`App::apply`].
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
    pub fn new(ports: P, agents: AgentDirectory, cfg: AppConfig) -> Self {
        App {
            ports,
            agents,
            cfg,
            ready: AtomicBool::new(true),
            shutting_down: AtomicBool::new(false),
        }
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

    async fn notify(&self, topic: Topic) {
        if let Err(e) = self.ports.wakeup().notify(topic).await {
            tracing::warn!(error = %e, "wakeup notify failed; consumers fall back to polling");
        }
    }

    /// Lists the configured agents with their live card data. A card that cannot be read
    /// in time yields an agent without `description` and `releases` (fail closed, ADR 0008).
    pub async fn list_agents(&self) -> Vec<AgentInfo> {
        let lookups = self.agents.iter().map(|entry| async move {
            let card = tokio::time::timeout(
                self.cfg.card_timeout,
                self.ports.agents().read_card(&entry.endpoint),
            )
            .await;
            let (description, releases) = match card {
                Ok(Ok(card)) => (card.description, card.releases),
                Ok(Err(e)) => {
                    tracing::warn!(agent = %entry.endpoint.id, error = %e, "agent card unreadable");
                    (None, None)
                }
                Err(_) => {
                    tracing::warn!(agent = %entry.endpoint.id, "agent card timed out");
                    (None, None)
                }
            };
            AgentInfo {
                id: entry.endpoint.id.clone(),
                name: entry.name.clone(),
                description,
                card_url: entry.endpoint.card_url.clone(),
                releases,
            }
        });
        futures::future::join_all(lookups).await
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
        let releases = match card {
            Ok(Ok(card)) => card.releases,
            Ok(Err(_)) | Err(_) => {
                return Err(AppError::Invalid(
                    "agent card unreachable; cannot validate release".to_owned(),
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
        validate_text(&req.text)?;
        if let Some(title) = &req.title
            && title.chars().count() > MAX_TITLE_CHARS
        {
            return Err(AppError::Invalid(format!(
                "title must be at most {MAX_TITLE_CHARS} characters"
            )));
        }
        self.validate_target(&req.target).await?;

        let id = ThreadId(self.ports.ids().new_id());
        let now = self.ports.clock().now();
        let (next, cmds) = transition(
            &ThreadState::Queued,
            &Input::UserMessage {
                user: user.clone(),
                text: req.text.clone(),
            },
        )?;
        let title = req
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| default_title(&req.text));
        let commit = self.build_commit(&req.target, next, cmds, None, None, now);
        let new = NewThreadRecord {
            id,
            owner: user.clone(),
            title,
            target: req.target,
            context_id: id.to_string(),
            now,
        };
        let (thread, _events) = self.ports.store().create_thread(new, commit).await?;
        self.notify(Topic::Thread(id)).await;
        self.notify(Topic::Outbox).await;
        Ok(thread)
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
                },
                None,
                None,
            )
            .await?;
        match outcome {
            ApplyOutcome::Applied { events, .. } => events
                .into_iter()
                .find(|e| e.kind() == EventKind::UserMessage)
                .ok_or_else(|| {
                    AppError::Store(StoreError::Corrupt("user_message missing".to_owned()))
                }),
            ApplyOutcome::Duplicate => Err(AppError::Store(StoreError::Corrupt(
                "unexpected duplicate".to_owned(),
            ))),
        }
    }

    /// Requests cancellation of the thread's running work. A finished thread is a no-op.
    pub async fn cancel(&self, user: &UserId, id: ThreadId) -> Result<(), AppError> {
        self.get_thread(user, id).await?;
        self.apply(id, Input::Cancel { user: user.clone() }, None, None)
            .await?;
        Ok(())
    }

    /// Turns commands into one store commit. The first appended event carries `key`, the
    /// following ones `key#1`, `key#2`, …
    fn build_commit(
        &self,
        target: &AgentTarget,
        new_state: ThreadState,
        cmds: Vec<Command>,
        key: Option<&str>,
        binding: Option<BindingUpdate>,
        now: Timestamp,
    ) -> Commit {
        let mut events = Vec::new();
        let mut outbox = Vec::new();
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
                Command::RequestCancel => outbox.push(NewOutbox {
                    id: orch_ports::OutboxId(self.ports.ids().new_id()),
                    payload: OutboxPayload::Cancel,
                }),
            }
        }
        Commit {
            new_state,
            events,
            outbox,
            binding,
            now,
        }
    }

    /// Applies `input` to the thread: `transition`, then one store commit, retried on version
    /// conflicts. `key` makes a replayed input idempotent; `binding` is persisted in the same
    /// transaction. Used by the API and by the dispatcher.
    pub async fn apply(
        &self,
        thread: ThreadId,
        input: Input,
        key: Option<String>,
        binding: Option<BindingUpdate>,
    ) -> Result<ApplyOutcome, AppError> {
        for _ in 0..self.cfg.max_commit_attempts {
            let record = self
                .ports
                .store()
                .get_thread(None, thread)
                .await?
                .ok_or(AppError::NotFound)?;
            let (next, cmds) = transition(&record.state, &input)?;
            let now = self.ports.clock().now();
            let commit = self.build_commit(
                &record.target,
                next,
                cmds,
                key.as_deref(),
                binding.clone(),
                now,
            );
            if commit.events.is_empty()
                && commit.outbox.is_empty()
                && commit.binding.is_none()
                && next == record.state
            {
                return Ok(ApplyOutcome::Applied {
                    thread: record,
                    events: Vec::new(),
                });
            }
            let has_outbox = !commit.outbox.is_empty();
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
                    return Ok(ApplyOutcome::Applied { thread: t, events });
                }
                Ok(CommitOutcome::Duplicate) => return Ok(ApplyOutcome::Duplicate),
                Err(StoreError::VersionConflict) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(AppError::Store(StoreError::VersionConflict))
    }

    /// Persists binding fields (task id, state, revision) without changing the thread.
    pub async fn record_binding(
        &self,
        thread: ThreadId,
        binding: BindingUpdate,
    ) -> Result<(), AppError> {
        for _ in 0..self.cfg.max_commit_attempts {
            let record = self
                .ports
                .store()
                .get_thread(None, thread)
                .await?
                .ok_or(AppError::NotFound)?;
            let commit = Commit {
                new_state: record.state,
                events: Vec::new(),
                outbox: Vec::new(),
                binding: Some(binding.clone()),
                now: self.ports.clock().now(),
            };
            match self
                .ports
                .store()
                .commit(thread, record.version, commit)
                .await
            {
                Ok(_) => return Ok(()),
                Err(StoreError::VersionConflict) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(AppError::Store(StoreError::VersionConflict))
    }

    /// Replays every event with `seq > after`, then streams live ones, without gaps or
    /// duplicates. Wakeups make it prompt; a periodic poll makes it correct without them.
    pub async fn event_stream(
        self: &Arc<Self>,
        user: &UserId,
        id: ThreadId,
        after: i64,
    ) -> Result<BoxStream<'static, Event>, AppError> {
        self.get_thread(user, id).await?;
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
            cursor: after.max(0),
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
                    Err(e) => tracing::warn!(error = %e, "event stream read failed; retrying"),
                }
                let tick = tokio::time::sleep_until(Instant::now() + st.app.cfg.stream_poll);
                tokio::pin!(tick);
                loop {
                    tokio::select! {
                        topic = st.wake.next(), if st.wake_open => match topic {
                            Some(Topic::Thread(t)) if t == st.id => break,
                            Some(Topic::Resync) => break,
                            Some(Topic::Thread(_) | Topic::Outbox) => {}
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
