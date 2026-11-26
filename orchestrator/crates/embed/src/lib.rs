//! The orchestrator as a library in an application (ADR 0076 decision 4): a composition root that
//! wires [`orch_app::App`] and its dispatcher to the SQLite thread store, with no HTTP server and no
//! `axum`, and offers the chat API's operations and the AG-UI projection as Rust calls.
//!
//! ```no_run
//! use orch_app::AgentDirectory;
//! use orch_core::{AgentId, UserId};
//! use orch_embed::Embedded;
//! use orch_ports::AgentEndpoint;
//!
//! # async fn run<A: orch_ports::AgentClient>(
//! #     agents: A,
//! #     directory: AgentDirectory,
//! # ) -> Result<(), Box<dyn std::error::Error>> {
//! let embed = Embedded::open("orch.sqlite", UserId::new("me@example.com"), directory, agents).await?;
//! let thread = embed
//!     .create_thread(&AgentId::new("helper"), "hello")
//!     .await?;
//! let events = embed.collect_agui(thread.id).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # What it composes
//!
//! | Port | Adapter here |
//! |---|---|
//! | `ThreadStore` (and `UsageLedger`, `ThreadSearch`, `PromptStore`, `UserToolServerStore`, `PendingAuthorizations`) | [`orch_store_sqlite::SqliteStore`] |
//! | `Wakeup` | [`orch_store_sqlite::SqliteWakeup`], in process |
//! | `AgentClient` | the caller's, generic ([`orch_ports::AgentClient`]) |
//! | `AgentRegistry` | the static [`orch_app::AgentDirectory`] the caller gives |
//! | `Clock`, `IdGen` | the system clock and UUIDv7 ids |
//! | `ChatModel` | the application's, through [`EmbedPorts`]; none by default: titles are the first words of the messages and no description task runs |
//! | `Authenticator` | [`orch_ports::RefuseAll`]: the application states the person, and no bearer is checked in process |
//! | `ArtifactStore` | none |
//! | `UserToolPorts` | off |
//!
//! It does not reimplement the application: a thread is created and a message submitted through
//! [`orch_app::App`], the dispatcher is [`orch_app::Dispatcher`], and the AG-UI stream folds the log
//! with [`orch_agui_projection::Projector`]. What it does not build yet is ADR 0076's local agent
//! adapter (slice E4: adam in process on adam's SQLite store), the application's title model, its
//! secret store and its filesystem artifact store.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Arc;

use futures::stream::{BoxStream, StreamExt};
use orch_agui_projection::{Audience, ChildMeta, Frame, Projector, ThreadMeta};
use orch_agui_proto as agui;
use orch_app::{
    AgentDirectory, App, AppConfig, AppError, ApplyOutcome, Dispatcher, DispatcherConfig, FeedItem,
    GateError, NewThread, TaskSettings,
};
use orch_core::client_version::ClientVersion;
use orch_core::{
    AgentId, AgentTarget, Event, EventKind, Input, Origin, TaskKind, ThreadId, ThreadRecord, UserId,
};
use orch_ports::{
    AgentClient, ArtifactStore, ChatModel, NoArtifacts, NoModel, NoUserTools, PortSet, RefuseAll,
    SystemClock, UserToolPorts, UuidV7Ids,
};
use orch_store_sqlite::{SqliteStore, SqliteWakeup};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// `CUSTOM` name of the event that says the application's limit is reached (status 402 of the
/// cloud adapter). The value is `{status: 402, code, message}`. A client that does not know it
/// loses nothing else: it is not part of the message sequence.
pub const CUSTOM_LIMIT_REACHED: &str = "vymalo.limit_reached";

/// Why an application refuses to start work: its person has used what they may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limit {
    /// A stable, machine-readable code the application matches on.
    pub code: String,
    /// What the person is told.
    pub message: String,
}

impl Limit {
    /// The AG-UI event that says it: one `CUSTOM` named [`CUSTOM_LIMIT_REACHED`].
    pub fn event(&self) -> agui::Event {
        agui::CustomEvent::new(
            CUSTOM_LIMIT_REACHED,
            serde_json::json!({"status": 402, "code": self.code, "message": self.message}),
        )
        .into()
    }
}

/// Decides, before work starts, whether the person may start it. The application implements it
/// (what is allowed is its own); the default allows everything.
pub trait Admission: std::fmt::Debug + Send + Sync {
    /// `Ok` to go on, or the [`Limit`] that stops it.
    ///
    /// # Errors
    /// The limit the person has reached.
    fn admit(&self, person: &UserId) -> Result<(), Limit>;
}

/// Events read per page when a whole thread is read or streamed.
const PAGE: u32 = 500;

/// The ports of an embedded application: SQLite, the caller's agent client and, unless the caller
/// supplies them through [`EmbedPorts`], no model, no artifact store and no servers of the person's
/// own. There is never an authenticator.
pub type Stack<A, M = NoModel, X = NoArtifacts, T = NoUserTools> = PortSet<
    SqliteStore,
    SqliteWakeup,
    A,
    SystemClock,
    UuidV7Ids,
    M,
    orch_ports::FixedRegistry,
    RefuseAll,
    X,
    T,
>;

/// The application's own pieces, handed to [`Embedded::open_with`] through ports that already exist.
///
/// Each field defaults to the refusing port, so `EmbedPorts::default()` is what [`Embedded::open`]
/// builds. The secret store enters through `user_tools`: the application builds the bundle over its
/// own `SecretStore` with `orch-credential-broker` (this crate does not depend on it, nor on
/// `axum`), and hands it here.
#[derive(Debug, Clone)]
pub struct EmbedPorts<M = NoModel, X = NoArtifacts, T = NoUserTools> {
    /// The model client the orchestrator asks for thread titles and descriptions (the app's
    /// gateway credentials live behind it).
    pub model: M,
    /// The file store behind the files agents hand over.
    pub artifacts: X,
    /// The person's own MCP servers and the credentials kept for them.
    pub user_tools: T,
    /// The utility tasks (titles, descriptions) that ask `model`. Empty by default: a task that is
    /// absent is off and the model is never asked (ADR 0035).
    pub tasks: BTreeMap<TaskKind, TaskSettings>,
    /// The lowest application version served (`server.minClientVersion` of a deployment): an
    /// operation made by a client that states a lower version, or none, is refused with
    /// [`EmbedError::ClientUpdateRequired`]. `None`: no gate.
    pub min_client_version: Option<ClientVersion>,
    /// Asked before a thread is created or a message is posted. `None`: always allowed.
    pub admission: Option<Arc<dyn Admission>>,
}

impl Default for EmbedPorts {
    fn default() -> Self {
        EmbedPorts {
            model: NoModel,
            artifacts: NoArtifacts,
            user_tools: NoUserTools::off(),
            tasks: BTreeMap::new(),
            min_client_version: None,
            admission: None,
        }
    }
}

/// What an embedded orchestrator failed to do.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    /// The SQLite store refused to open, migrate or read.
    #[error(transparent)]
    Store(#[from] orch_ports::StoreError),
    /// The application refused the operation (a permission, a missing thread, invalid input).
    #[error(transparent)]
    App(#[from] AppError),
    /// The verification gate the configuration asks for cannot be honoured.
    #[error(transparent)]
    Gate(#[from] GateError),
    /// The client's version is below the minimum (426): the application should say "update".
    #[error("update the application: version {min} or later is needed")]
    ClientUpdateRequired {
        /// The lowest version served.
        min: ClientVersion,
    },
    /// The person has reached a limit (402); [`Limit::event`] is its AG-UI event.
    #[error("limit reached: {}", .0.message)]
    LimitReached(Limit),
}

/// The orchestrator embedded in an application: one thread log in SQLite, one dispatcher on the
/// caller's runtime, and the person the application states.
///
/// Dropping it cancels the dispatcher; [`shutdown`](Self::shutdown) waits for it to end.
pub struct Embedded<
    A: AgentClient,
    M: ChatModel = NoModel,
    X: ArtifactStore = NoArtifacts,
    T: UserToolPorts = NoUserTools,
> {
    app: Arc<App<Stack<A, M, X, T>>>,
    person: UserId,
    shutdown: CancellationToken,
    dispatcher: JoinHandle<()>,
    min_client_version: Option<ClientVersion>,
    client_version: Option<ClientVersion>,
    admission: Option<Arc<dyn Admission>>,
}

impl<A: AgentClient> Embedded<A> {
    /// Opens (or creates) the SQLite file at `path`, migrates it, builds the application over the
    /// given agents, and starts its dispatcher on the current Tokio runtime.
    ///
    /// The caller gives the person every operation is made for, the static directory of agents (the
    /// same entries must be the ones `agents` serves), and the `AgentClient` adapter.
    ///
    /// # Errors
    /// [`EmbedError::Store`] when the file cannot be opened (including a second process on the same
    /// file) or migrated, and [`EmbedError::Gate`] when the application's gate cannot be honoured.
    pub async fn open(
        path: impl AsRef<Path>,
        person: UserId,
        directory: AgentDirectory,
        agents: A,
    ) -> Result<Self, EmbedError> {
        Embedded::open_with(path, person, directory, agents, EmbedPorts::default()).await
    }
}

impl<A: AgentClient, M: ChatModel, X: ArtifactStore, T: UserToolPorts> Embedded<A, M, X, T> {
    /// [`open`](Embedded::open) with the application's own model client, file store and
    /// user-tool bundle in place of the refusing ones.
    ///
    /// # Errors
    /// As [`open`](Embedded::open).
    pub async fn open_with(
        path: impl AsRef<Path>,
        person: UserId,
        directory: AgentDirectory,
        agents: A,
        parts: EmbedPorts<M, X, T>,
    ) -> Result<Self, EmbedError> {
        let store = SqliteStore::open(path)?;
        store.migrate().await?;
        let registry = directory.fixed_registry();
        let ports = Stack {
            store,
            wakeup: SqliteWakeup::new(),
            agents,
            clock: SystemClock,
            ids: UuidV7Ids,
            model: parts.model,
            auth: RefuseAll,
            registry,
            artifacts: parts.artifacts,
            user_tools: parts.user_tools,
        };
        let config = AppConfig {
            stream_poll: std::time::Duration::from_millis(50),
            tasks: parts.tasks,
            ..AppConfig::default()
        };
        let app = Arc::new(App::new(ports, directory, config)?);
        let shutdown = CancellationToken::new();
        let dispatcher = Dispatcher::new(
            Arc::clone(&app),
            DispatcherConfig {
                poll_interval: std::time::Duration::from_millis(50),
                ..DispatcherConfig::default()
            },
            format!("embed:{person}"),
        );
        let dispatcher = tokio::spawn(dispatcher.run(shutdown.child_token()));
        Ok(Embedded {
            app,
            person,
            shutdown,
            dispatcher,
            min_client_version: parts.min_client_version,
            client_version: None,
            admission: parts.admission,
        })
    }

    /// The application, for an operation this root does not wrap.
    pub fn app(&self) -> &Arc<App<Stack<A, M, X, T>>> {
        &self.app
    }

    /// States the version of the application making the calls (`1.4.0`). Under a minimum
    /// ([`EmbedPorts::min_client_version`]) an unreadable version is as good as none.
    #[must_use]
    pub fn with_client_version(mut self, version: &str) -> Self {
        self.client_version = version.trim().parse().ok();
        self
    }

    /// The 426 gate: every operation passes it.
    fn gate(&self) -> Result<(), EmbedError> {
        match (&self.min_client_version, &self.client_version) {
            (Some(min), Some(stated)) if stated >= min => Ok(()),
            (Some(min), _) => Err(EmbedError::ClientUpdateRequired { min: min.clone() }),
            (None, _) => Ok(()),
        }
    }

    /// The 402 check: starting work asks the application's admission.
    fn admit(&self) -> Result<(), EmbedError> {
        match &self.admission {
            Some(admission) => admission
                .admit(&self.person)
                .map_err(EmbedError::LimitReached),
            None => Ok(()),
        }
    }

    /// The person every operation is made for.
    pub fn person(&self) -> &UserId {
        &self.person
    }

    /// Creates a thread whose first message is `text`, queued for the dispatcher.
    pub async fn create_thread(
        &self,
        agent: &AgentId,
        text: impl Into<String>,
    ) -> Result<ThreadRecord, EmbedError> {
        self.gate()?;
        self.admit()?;
        let request = NewThread {
            title: None,
            target: AgentTarget {
                agent_id: agent.clone(),
                release: None,
            },
            text: text.into(),
        };
        Ok(self.app.create_thread(&self.person, request).await?)
    }

    /// Appends `text` to a thread and queues its delegation.
    pub async fn post_message(
        &self,
        thread: ThreadId,
        text: impl Into<String>,
    ) -> Result<Event, EmbedError> {
        self.gate()?;
        self.admit()?;
        Ok(self
            .app
            .post_message(&self.person, thread, text.into())
            .await?)
    }

    /// [`post_message`](Self::post_message) under the client's own message id: the application
    /// retries a send it is unsure of with the same id, and the thread holds the message once.
    /// The answer to the retry is the message already logged.
    pub async fn post_message_with_id(
        &self,
        thread: ThreadId,
        client_message_id: &str,
        text: impl Into<String>,
    ) -> Result<Event, EmbedError> {
        self.gate()?;
        let input = Input::UserMessage {
            user: self.person.clone(),
            text: text.into(),
            message_id: Some(client_message_id.to_owned()),
            run_id: None,
            origin: Origin::default(),
            catalog: None,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
            locale: None,
            pick: None,
        };
        // a retry is not new work: only the first send asks the admission
        let key = format!("embed-message:{thread}:{client_message_id}");
        if let Some(logged) = self.logged_message(thread, client_message_id).await? {
            return Ok(logged);
        }
        self.admit()?;
        match self
            .app
            .submit(&self.person, thread, input, Some(key))
            .await?
        {
            ApplyOutcome::Applied { events, .. } => events
                .into_iter()
                .find(|e| e.kind() == EventKind::UserMessage)
                .ok_or_else(|| {
                    AppError::internal("the user message is missing from its commit").into()
                }),
            ApplyOutcome::Duplicate => self
                .logged_message(thread, client_message_id)
                .await?
                .ok_or_else(|| AppError::internal("a duplicate message is not in the log").into()),
            ApplyOutcome::Fenced => {
                Err(AppError::internal("a commit without a lease was reported as fenced").into())
            }
        }
    }

    async fn logged_message(
        &self,
        thread: ThreadId,
        client_message_id: &str,
    ) -> Result<Option<Event>, EmbedError> {
        Ok(self.events(thread).await?.into_iter().find(|event| {
            matches!(&event.body, orch_core::EventBody::UserMessage(m)
                if m.message_id.as_deref() == Some(client_message_id))
        }))
    }

    /// The thread as the log holds it.
    pub async fn read_thread(&self, thread: ThreadId) -> Result<ThreadRecord, EmbedError> {
        self.gate()?;
        Ok(self.app.get_thread(&self.person, thread).await?)
    }

    /// Every event of the thread, oldest first.
    pub async fn events(&self, thread: ThreadId) -> Result<Vec<Event>, EmbedError> {
        self.gate()?;
        let mut all = Vec::new();
        loop {
            let after = all.last().map_or(0, |event: &Event| event.seq);
            let page = self
                .app
                .list_events(&self.person, thread, after, PAGE)
                .await?;
            let full = page.len() >= PAGE as usize;
            all.extend(page);
            if !full {
                return Ok(all);
            }
        }
    }

    /// The thread's AG-UI events as they are written: the viewer projection of the log, from the
    /// beginning, ending at the run's `RUN_FINISHED` or `RUN_ERROR`.
    ///
    /// This is the same fold `orch-surface-agui` writes over an SSE response; live text that is
    /// not in the log (ADR 0027) is not mixed in, so a caller sees every frame once it is committed.
    pub async fn stream_agui(
        &self,
        thread: ThreadId,
    ) -> Result<BoxStream<'static, agui::Event>, EmbedError> {
        Ok(self
            .stream_agui_from(thread, 0)
            .await?
            .map(|frame| frame.event)
            .boxed())
    }

    /// Replay from a sequence number: the frames after the log event `after` (the `resume_id` of
    /// the last frame the application holds; 0 is the start). What the log says up to `after` is
    /// folded without being said, so a resumed stream first re-opens the run that is open
    /// (`RUN_STARTED` and its open invocations, [`Projector::resume_preamble`]) and then goes on
    /// exactly where the application stopped. A frame with `resume_id: Some(seq)` is a point to
    /// resume from.
    pub async fn stream_agui_from(
        &self,
        thread: ThreadId,
        after: i64,
    ) -> Result<BoxStream<'static, Frame>, EmbedError> {
        self.gate()?;
        let record = self.read_thread(thread).await?;
        let projector = Projector::new(meta_of(&record));
        let feed = self.app.thread_feed(&self.person, thread, 0).await?;
        let state = AguiState {
            feed,
            projector,
            pending: VecDeque::new(),
            done: false,
            after,
            writing: after <= 0,
        };
        Ok(futures::stream::unfold(state, |mut state| async move {
            loop {
                if let Some(frame) = state.pending.pop_front() {
                    if is_terminal(&frame.event) {
                        state.done = true;
                        state.pending.clear();
                    }
                    return Some((frame, state));
                }
                if state.done {
                    return None;
                }
                match state.feed.next().await? {
                    FeedItem::Event(event) => {
                        let frames = state.projector.apply(&event, Audience::Viewer);
                        if state.writing {
                            state.pending.extend(frames);
                        } else if event.seq > state.after {
                            state.writing = true;
                            state.pending.extend(state.projector.resume_preamble());
                            state.pending.extend(frames);
                        }
                    }
                    // The log is the truth; live text is a latency optimisation an embedded
                    // application reads from the events it is given (ADR 0027).
                    FeedItem::Live(_) => {}
                }
            }
        })
        .boxed())
    }

    /// [`stream_agui`](Self::stream_agui), collected to its end: every AG-UI event of the thread so
    /// far, stopping after the first run's terminal event.
    pub async fn collect_agui(&self, thread: ThreadId) -> Result<Vec<agui::Event>, EmbedError> {
        let mut stream = self.stream_agui(thread).await?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        Ok(events)
    }

    /// Cancels the dispatcher and waits for it to stop.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        let _ = (&mut self.dispatcher).await;
    }
}

impl<A: AgentClient, M: ChatModel, X: ArtifactStore, T: UserToolPorts> Drop
    for Embedded<A, M, X, T>
{
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.dispatcher.abort();
    }
}

/// The projection's fold over the thread's feed.
struct AguiState {
    feed: BoxStream<'static, FeedItem>,
    projector: Projector,
    pending: VecDeque<Frame>,
    done: bool,
    after: i64,
    writing: bool,
}

/// A run ends with `RUN_FINISHED` or `RUN_ERROR`.
fn is_terminal(event: &agui::Event) -> bool {
    matches!(
        event,
        agui::Event::RunFinished(_) | agui::Event::RunError(_)
    )
}

/// The thread as the AG-UI projection reads it.
fn meta_of(thread: &ThreadRecord) -> ThreadMeta {
    ThreadMeta {
        thread_id: thread.id,
        title: thread.title.clone(),
        description: thread.description.clone(),
        target: thread.target.clone(),
        gate: thread.job.gate.clone(),
        child: thread.job.child.as_ref().map(|child| ChildMeta {
            agent: child.agent.clone(),
            keep_open: child.keep_open,
        }),
    }
}
