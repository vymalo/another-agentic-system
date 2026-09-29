//! An in-process A2A 1.0 agent for tests, built on `a2a-server-lf`'s `DefaultRequestHandler`.
//!
//! It serves a card at `/.well-known/agent-card.json` (public) and JSON-RPC at `/a2a`
//! (optionally behind a bearer check that answers 401 without a JSON-RPC body, like a real
//! proxy). Like the SDK's handler, task execution continues when the client disconnects, and
//! `SubscribeToTask` works only while a task executes.
//!
//! Behaviour is chosen by the first word of the user's message:
//!
//! | word | behaviour |
//! |---|---|
//! | `echo` (or anything else) | `working`, artifact `echo: <text>` with a PR URL, `completed` |
//! | `ask` | `working`, `input-required("Which branch?")`; the follow-up on the same task: `working`, artifact `answered: <text>`, `completed` |
//! | `gate` | `working`, then waits for [`FakeAgent::release_gate`], then artifact and `completed` |
//! | `slow` | `working`, then runs until cancelled |
//! | `chunks` | `working`, one artifact sent as three appended chunks, `completed` |
//! | `fail` | `working`, `failed("scripted failure")` |
//! | `talk` | `working`, `working("Reading the repository")`, an agent `Message` "Plan: add a test", artifact `echo: <text>`, `completed` |
//!
//! With [`FakeAgentOptions::releases`] the card declares the release-channels extension, a new
//! task starts with a `Task` frame whose metadata records `{requested, revision}`, every event
//! echoes that metadata, and an unknown release fails the task (never the default).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use a2a::{
    A2AError, AgentCapabilities, AgentCard, AgentExtension, AgentInterface, Artifact,
    CancelTaskRequest, DeleteTaskPushNotificationConfigRequest, GetExtendedAgentCardRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest,
    ListTaskPushNotificationConfigsResponse, ListTasksRequest, ListTasksResponse, Message, Part,
    Role, SendMessageRequest, SendMessageResponse, StreamResponse, SubscribeToTaskRequest,
    TRANSPORT_PROTOCOL_JSONRPC, Task, TaskArtifactUpdateEvent, TaskPushNotificationConfig,
    TaskState, TaskStatus, TaskStatusUpdateEvent,
};
use a2a_server::{
    AgentExecutor, DefaultRequestHandler, ExecutorContext, InMemoryTaskStore, RequestHandler,
    ServiceParams, StaticAgentCard,
};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::stream::BoxStream;
use orch_core::AgentId;
use orch_ports::AgentEndpoint;
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;

/// The release-channels extension URI (kept literal: test support must not depend on the adapter).
pub const EXTENSION_URI: &str = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

/// The URL every finished script reports as its artifact.
pub const PR_URL: &str = "https://github.com/acme/demo/pull/1";

/// Release channels the fake card declares.
#[derive(Debug, Clone)]
pub struct FakeReleases {
    /// Channel used when the message names none.
    pub default_channel: String,
    /// Channel name to revision name.
    pub channels: Vec<(String, String)>,
    /// Invocable revisions, newest first.
    pub revisions: Vec<String>,
}

impl FakeReleases {
    /// The example of the release-channels v1 spec.
    pub fn sample() -> Self {
        FakeReleases {
            default_channel: "production".to_owned(),
            channels: vec![
                ("production".to_owned(), "coder-r47".to_owned()),
                ("staging".to_owned(), "coder-r51".to_owned()),
                ("latest".to_owned(), "coder-r53".to_owned()),
            ],
            revisions: vec![
                "coder-r53".to_owned(),
                "coder-r51".to_owned(),
                "coder-r47".to_owned(),
            ],
        }
    }

    fn resolve(&self, selector: &str) -> Option<String> {
        self.channels
            .iter()
            .find(|(name, _)| name == selector)
            .map(|(_, rev)| rev.clone())
            .or_else(|| self.revisions.iter().find(|r| *r == selector).cloned())
    }

    fn params(&self) -> HashMap<String, Value> {
        let channels: Map<String, Value> = self
            .channels
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        let revisions: Vec<Value> = self
            .revisions
            .iter()
            .map(|r| json!({ "name": r, "createdAt": "2026-09-28T09:10:00Z" }))
            .collect();
        HashMap::from([
            ("service".to_owned(), json!("coder")),
            ("defaultChannel".to_owned(), json!(self.default_channel)),
            ("channels".to_owned(), Value::Object(channels)),
            ("revisions".to_owned(), Value::Array(revisions)),
        ])
    }
}

/// How to start a [`FakeAgent`].
#[derive(Debug, Clone)]
pub struct FakeAgentOptions {
    /// Require `Authorization: Bearer <token>` on the RPC endpoint (the card stays public).
    pub bearer: Option<String>,
    /// Declare the release-channels extension in the card.
    pub releases: Option<FakeReleases>,
    /// `false` makes `SubscribeToTask` answer `UNSUPPORTED_OPERATION`, forcing `GetTask` polling.
    pub resubscribe: bool,
    /// Where to listen. `None` (the default) binds `127.0.0.1:0`, a free port.
    pub bind: Option<SocketAddr>,
}

impl Default for FakeAgentOptions {
    fn default() -> Self {
        FakeAgentOptions {
            bearer: None,
            releases: None,
            resubscribe: true,
            bind: None,
        }
    }
}

/// Which executor entry point ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// A message was executed.
    Execute,
    /// A `CancelTask` was executed.
    Cancel,
}

/// What the fake agent's executor observed.
#[derive(Debug, Clone)]
pub struct Call {
    /// Entry point.
    pub kind: CallKind,
    /// A2A task id.
    pub task_id: String,
    /// A2A context id.
    pub context_id: String,
    /// The user message's id (`Execute` only).
    pub message_id: Option<String>,
    /// The user message's text.
    pub text: String,
    /// The task was waiting for input and this message continues it.
    pub resuming: bool,
    /// Values of the `A2A-Extensions` request header.
    pub extensions_header: Vec<String>,
    /// `metadata[<extension URI>].release` of the message.
    pub release: Option<String>,
    /// The `Authorization` request header.
    pub authorization: Option<String>,
}

impl Call {
    /// The request activated the release-channels extension.
    pub fn activates_release_channels(&self) -> bool {
        self.extensions_header
            .iter()
            .any(|h| h.split(',').any(|e| e.trim() == EXTENSION_URI))
    }
}

struct Shared {
    calls: Mutex<Vec<Call>>,
    gate: Notify,
    cancels: Mutex<HashMap<String, Arc<Notify>>>,
    revisions: Mutex<HashMap<String, String>>,
    /// Tasks waiting for the answer to an `ask`.
    asking: Mutex<HashSet<String>>,
    artifact_seq: AtomicU64,
    unauthorized: AtomicUsize,
    rpcs: Mutex<HashMap<String, usize>>,
    releases: Option<FakeReleases>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running fake agent. Dropping it stops the server.
pub struct FakeAgent {
    base_url: String,
    shared: Arc<Shared>,
    server: JoinHandle<()>,
}

impl Drop for FakeAgent {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl FakeAgent {
    /// Starts an agent on [`FakeAgentOptions::bind`] (default `127.0.0.1:0`).
    pub async fn spawn(opts: FakeAgentOptions) -> Self {
        let bind = opts
            .bind
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)));
        let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let shared = Arc::new(Shared {
            calls: Mutex::new(Vec::new()),
            gate: Notify::new(),
            cancels: Mutex::new(HashMap::new()),
            revisions: Mutex::new(HashMap::new()),
            asking: Mutex::new(HashSet::new()),
            artifact_seq: AtomicU64::new(0),
            unauthorized: AtomicUsize::new(0),
            rpcs: Mutex::new(HashMap::new()),
            releases: opts.releases.clone(),
        });
        let capabilities = AgentCapabilities {
            streaming: Some(true),
            ..AgentCapabilities::default()
        };
        let handler =
            DefaultRequestHandler::new(Executor(Arc::clone(&shared)), InMemoryTaskStore::new())
                .with_capabilities(capabilities.clone());
        let rpc = a2a_server::jsonrpc::jsonrpc_router(Arc::new(Front {
            inner: handler,
            resubscribe: opts.resubscribe,
            shared: Arc::clone(&shared),
        }));
        let rpc = match opts.bearer.clone() {
            Some(token) => rpc.layer(axum::middleware::from_fn_with_state(
                Arc::new(Guard {
                    header: format!("Bearer {token}"),
                    shared: Arc::clone(&shared),
                }),
                require_bearer,
            )),
            None => rpc,
        };
        let card = card(&base_url, capabilities, opts.releases.as_ref());
        let app =
            axum::Router::new()
                .nest("/a2a", rpc)
                .merge(a2a_server::agent_card::agent_card_router(Arc::new(
                    StaticAgentCard::new(card),
                )));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        FakeAgent {
            base_url,
            shared,
            server,
        }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The card document URL.
    pub fn card_url(&self) -> String {
        format!("{}/.well-known/agent-card.json", self.base_url)
    }

    /// An orchestrator endpoint pointing at this agent.
    pub fn endpoint(&self, id: &str, bearer: Option<&str>) -> AgentEndpoint {
        AgentEndpoint {
            id: AgentId::new(id),
            card_url: self.card_url(),
            bearer: bearer.map(str::to_owned),
        }
    }

    /// Everything the executor saw, in order.
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.shared.calls).clone()
    }

    /// The executed messages.
    pub fn executions(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| c.kind == CallKind::Execute)
            .collect()
    }

    /// The executed cancels.
    pub fn cancels(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| c.kind == CallKind::Cancel)
            .collect()
    }

    /// Lets one waiting `gate` task continue (a permit is kept if none waits yet).
    pub fn release_gate(&self) {
        self.shared.gate.notify_one();
    }

    /// How often the RPC `method` (`send_streaming_message`, `subscribe_to_task`, `get_task`,
    /// `list_tasks`, `cancel_task`) was received (also when it was refused).
    pub fn rpc_count(&self, method: &str) -> usize {
        lock(&self.shared.rpcs).get(method).copied().unwrap_or(0)
    }

    /// RPC requests refused by the bearer check.
    pub fn unauthorized_requests(&self) -> usize {
        self.shared.unauthorized.load(Ordering::SeqCst)
    }

    /// Stops the server: the agent becomes unreachable.
    pub fn stop(&self) {
        self.server.abort();
    }
}

fn card(base: &str, capabilities: AgentCapabilities, releases: Option<&FakeReleases>) -> AgentCard {
    let extensions = releases.map(|r| {
        vec![AgentExtension {
            uri: EXTENSION_URI.to_owned(),
            description: Some("Select a release channel or an exact revision.".to_owned()),
            required: Some(false),
            params: Some(r.params()),
        }]
    });
    AgentCard {
        name: "fake-agent".to_owned(),
        description: "in-process fake A2A agent".to_owned(),
        version: "1.0.0".to_owned(),
        supported_interfaces: vec![AgentInterface::new(
            format!("{base}/a2a"),
            TRANSPORT_PROTOCOL_JSONRPC,
        )],
        capabilities: AgentCapabilities {
            extensions,
            ..capabilities
        },
        default_input_modes: vec!["text/plain".to_owned()],
        default_output_modes: vec!["text/plain".to_owned()],
        skills: vec![],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: None,
        security_requirements: None,
        signatures: None,
    }
}

struct Guard {
    header: String,
    shared: Arc<Shared>,
}

async fn require_bearer(State(guard): State<Arc<Guard>>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == guard.header);
    if ok {
        next.run(req).await
    } else {
        guard.shared.unauthorized.fetch_add(1, Ordering::SeqCst);
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// Wraps the default handler: counts RPCs, and can refuse `SubscribeToTask`.
struct Front {
    inner: DefaultRequestHandler,
    resubscribe: bool,
    shared: Arc<Shared>,
}

impl Front {
    fn seen(&self, method: &str) {
        *lock(&self.shared.rpcs)
            .entry(method.to_owned())
            .or_insert(0) += 1;
    }
}

#[async_trait::async_trait]
impl RequestHandler for Front {
    async fn send_message(
        &self,
        params: &ServiceParams,
        req: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2AError> {
        {
            self.seen("send_message");
            self.inner.send_message(params, req).await
        }
    }

    async fn send_streaming_message(
        &self,
        params: &ServiceParams,
        req: SendMessageRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        {
            self.seen("send_streaming_message");
            self.inner.send_streaming_message(params, req).await
        }
    }

    async fn get_task(
        &self,
        params: &ServiceParams,
        req: GetTaskRequest,
    ) -> Result<Task, A2AError> {
        {
            self.seen("get_task");
            self.inner.get_task(params, req).await
        }
    }

    async fn list_tasks(
        &self,
        params: &ServiceParams,
        req: ListTasksRequest,
    ) -> Result<ListTasksResponse, A2AError> {
        {
            self.seen("list_tasks");
            self.inner.list_tasks(params, req).await
        }
    }

    async fn cancel_task(
        &self,
        params: &ServiceParams,
        req: CancelTaskRequest,
    ) -> Result<Task, A2AError> {
        {
            self.seen("cancel_task");
            self.inner.cancel_task(params, req).await
        }
    }

    async fn subscribe_to_task(
        &self,
        params: &ServiceParams,
        req: SubscribeToTaskRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        self.seen("subscribe_to_task");
        if !self.resubscribe {
            return Err(A2AError::unsupported_operation(
                "this agent does not support resubscription",
            ));
        }
        self.inner.subscribe_to_task(params, req).await
    }

    async fn create_push_config(
        &self,
        params: &ServiceParams,
        req: TaskPushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        self.inner.create_push_config(params, req).await
    }

    async fn get_push_config(
        &self,
        params: &ServiceParams,
        req: GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        self.inner.get_push_config(params, req).await
    }

    async fn list_push_configs(
        &self,
        params: &ServiceParams,
        req: ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, A2AError> {
        self.inner.list_push_configs(params, req).await
    }

    async fn delete_push_config(
        &self,
        params: &ServiceParams,
        req: DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2AError> {
        self.inner.delete_push_config(params, req).await
    }

    async fn get_extended_agent_card(
        &self,
        params: &ServiceParams,
        req: GetExtendedAgentCardRequest,
    ) -> Result<AgentCard, A2AError> {
        self.inner.get_extended_agent_card(params, req).await
    }
}

// ------------------------------------------------------------------ executor

type Metadata = Option<HashMap<String, Value>>;

struct Executor(Arc<Shared>);

/// What a task's events carry: the ids and the release echo.
#[derive(Clone)]
struct TaskCtx {
    task_id: String,
    context_id: String,
    metadata: Metadata,
}

impl TaskCtx {
    fn status(&self, state: TaskState, text: Option<&str>) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: self.task_id.clone(),
            context_id: self.context_id.clone(),
            status: TaskStatus {
                state,
                message: text.map(|t| {
                    let mut m = Message::new(Role::Agent, vec![Part::text(t)]);
                    m.task_id = Some(self.task_id.clone());
                    m.context_id = Some(self.context_id.clone());
                    m
                }),
                timestamp: None,
            },
            metadata: self.metadata.clone(),
        })
    }

    /// A standalone agent `Message` frame (not a status message).
    fn agent_message(&self, text: &str) -> StreamResponse {
        let mut m = Message::new(Role::Agent, vec![Part::text(text)]);
        m.task_id = Some(self.task_id.clone());
        m.context_id = Some(self.context_id.clone());
        m.metadata = self.metadata.clone();
        StreamResponse::Message(m)
    }

    fn artifact(
        &self,
        artifact_id: &str,
        name: &str,
        parts: Vec<Part>,
        append: bool,
        last_chunk: Option<bool>,
    ) -> StreamResponse {
        StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: self.task_id.clone(),
            context_id: self.context_id.clone(),
            artifact: Artifact {
                artifact_id: artifact_id.to_owned(),
                name: Some(name.to_owned()),
                description: None,
                parts,
                metadata: None,
                extensions: None,
            },
            append: append.then_some(true),
            last_chunk,
            metadata: self.metadata.clone(),
        })
    }
}

fn text_of(message: Option<&Message>) -> String {
    message
        .map(|m| {
            m.parts
                .iter()
                .filter_map(Part::as_text)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn requested_release(message: Option<&Message>) -> Option<String> {
    message?
        .metadata
        .as_ref()?
        .get(EXTENSION_URI)?
        .get("release")?
        .as_str()
        .map(str::to_owned)
}

async fn emit(
    tx: &mpsc::Sender<Result<StreamResponse, A2AError>>,
    ev: StreamResponse,
) -> Option<()> {
    tx.send(Ok(ev)).await.ok()
}

impl Shared {
    fn record(&self, ctx: &ExecutorContext, kind: CallKind, resuming: bool) {
        let header = |name: &str| ctx.service_params.get(name).cloned().unwrap_or_default();
        lock(&self.calls).push(Call {
            kind,
            task_id: ctx.task_id.clone(),
            context_id: ctx.context_id.clone(),
            message_id: ctx.message.as_ref().map(|m| m.message_id.clone()),
            text: text_of(ctx.message.as_ref()),
            resuming,
            extensions_header: header("a2a-extensions"),
            release: requested_release(ctx.message.as_ref()),
            authorization: header("authorization").first().cloned(),
        });
    }

    /// The revision serving `task`: fixed when the task starts (resolved once, per the
    /// extension), `Err` for an unknown release.
    fn revision_for(&self, task: &str, release: Option<&str>) -> Result<Option<String>, String> {
        let Some(releases) = &self.releases else {
            return Ok(None);
        };
        let mut known = lock(&self.revisions);
        if let Some(rev) = known.get(task) {
            return Ok(Some(rev.clone()));
        }
        let selector = release.unwrap_or(&releases.default_channel);
        let rev = releases
            .resolve(selector)
            .ok_or_else(|| format!("unknown release: {selector}"))?;
        known.insert(task.to_owned(), rev.clone());
        Ok(Some(rev))
    }

    fn next_artifact_id(&self) -> String {
        format!(
            "artifact-{}",
            self.artifact_seq.fetch_add(1, Ordering::SeqCst) + 1
        )
    }
}

fn echo_parts(text: &str) -> Vec<Part> {
    vec![Part::text(text), Part::url(PR_URL)]
}

async fn script(
    shared: Arc<Shared>,
    tx: mpsc::Sender<Result<StreamResponse, A2AError>>,
    ctx: TaskCtx,
    text: String,
    resuming: bool,
    cancel: Arc<Notify>,
) -> Option<()> {
    emit(&tx, ctx.status(TaskState::Working, None)).await?;
    // The answer to an `ask` continues the `ask` script whatever it says.
    let answering = resuming && lock(&shared.asking).remove(&ctx.task_id);
    let word = if answering {
        "ask"
    } else {
        text.split_whitespace().next().unwrap_or("")
    };
    let finish = |id: String, body: String| {
        let artifact = ctx.artifact(&id, "result", echo_parts(&body), false, None);
        (artifact, ctx.status(TaskState::Completed, None))
    };
    match word {
        "fail" => emit(&tx, ctx.status(TaskState::Failed, Some("scripted failure"))).await?,
        "ask" if !answering => {
            lock(&shared.asking).insert(ctx.task_id.clone());
            emit(
                &tx,
                ctx.status(TaskState::InputRequired, Some("Which branch?")),
            )
            .await?;
        }
        "ask" => {
            let (a, done) = finish(shared.next_artifact_id(), format!("answered: {text}"));
            emit(&tx, a).await?;
            emit(&tx, done).await?;
        }
        "talk" => {
            emit(
                &tx,
                ctx.status(TaskState::Working, Some("Reading the repository")),
            )
            .await?;
            emit(&tx, ctx.agent_message("Plan: add a test")).await?;
            let (a, done) = finish(shared.next_artifact_id(), format!("echo: {text}"));
            emit(&tx, a).await?;
            emit(&tx, done).await?;
        }
        "gate" => {
            shared.gate.notified().await;
            let (a, done) = finish(shared.next_artifact_id(), format!("echo: {text}"));
            emit(&tx, a).await?;
            emit(&tx, done).await?;
        }
        "slow" => {
            // Runs until CancelTask; the handler publishes the Canceled status itself.
            cancel.notified().await;
        }
        "chunks" => {
            let id = shared.next_artifact_id();
            emit(
                &tx,
                ctx.artifact(&id, "log", vec![Part::text("one")], false, Some(false)),
            )
            .await?;
            emit(
                &tx,
                ctx.artifact(&id, "log", vec![Part::text("two")], true, Some(false)),
            )
            .await?;
            emit(
                &tx,
                ctx.artifact(&id, "log", vec![Part::text("three")], true, Some(true)),
            )
            .await?;
            emit(&tx, ctx.status(TaskState::Completed, None)).await?;
        }
        _ => {
            let (a, done) = finish(shared.next_artifact_id(), format!("echo: {text}"));
            emit(&tx, a).await?;
            emit(&tx, done).await?;
        }
    }
    Some(())
}

impl AgentExecutor for Executor {
    fn execute(
        &self,
        ctx: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let shared = Arc::clone(&self.0);
        let resuming = ctx
            .stored_task
            .as_ref()
            .is_some_and(|t| t.status.state == TaskState::InputRequired);
        shared.record(&ctx, CallKind::Execute, resuming);

        let (task_id, context_id) = ctx.task_info();
        let text = text_of(ctx.message.as_ref());
        let release = requested_release(ctx.message.as_ref());
        let revision = shared.revision_for(&task_id, release.as_deref());
        let starts = !resuming
            && ctx
                .stored_task
                .as_ref()
                .is_none_or(|t| t.status.state == TaskState::Submitted);
        let cancel = Arc::new(Notify::new());
        lock(&shared.cancels).insert(task_id.clone(), Arc::clone(&cancel));
        let user_message = ctx.message.clone();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            let mut meta = Map::new();
            if let Ok(Some(rev)) = &revision {
                if let Some(r) = &release {
                    meta.insert("requested".to_owned(), json!(r));
                }
                meta.insert("revision".to_owned(), json!(rev));
            }
            let metadata: Metadata = (!meta.is_empty())
                .then(|| HashMap::from([(EXTENSION_URI.to_owned(), Value::Object(meta))]));
            let task = TaskCtx {
                task_id: task_id.clone(),
                context_id: context_id.clone(),
                metadata: metadata.clone(),
            };
            let outcome = async {
                match revision {
                    Err(why) => emit(&tx, task.status(TaskState::Failed, Some(&why))).await,
                    Ok(rev) => {
                        if starts && rev.is_some() {
                            // The task's metadata records which revision ran (extension rule 3).
                            let snapshot = Task {
                                id: task_id.clone(),
                                context_id: context_id.clone(),
                                status: TaskStatus {
                                    state: TaskState::Submitted,
                                    message: None,
                                    timestamp: None,
                                },
                                artifacts: None,
                                history: user_message.map(|m| vec![m]),
                                metadata,
                            };
                            emit(&tx, StreamResponse::Task(snapshot)).await?;
                        }
                        script(
                            Arc::clone(&shared),
                            tx.clone(),
                            task,
                            text,
                            resuming,
                            cancel,
                        )
                        .await
                    }
                }
            };
            let _ = outcome.await;
            lock(&shared.cancels).remove(&task_id);
        });
        Box::pin(ReceiverStream::new(rx))
    }

    fn cancel(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        self.0.record(&ctx, CallKind::Cancel, false);
        if let Some(n) = lock(&self.0.cancels).remove(&ctx.task_id) {
            n.notify_one();
        }
        let (task_id, context_id) = ctx.task_info();
        let metadata = self
            .0
            .revision_for(&task_id, None)
            .ok()
            .flatten()
            .map(|rev| HashMap::from([(EXTENSION_URI.to_owned(), json!({ "revision": rev }))]));
        let task = TaskCtx {
            task_id,
            context_id,
            metadata,
        };
        Box::pin(futures::stream::once(async move {
            Ok(task.status(TaskState::Canceled, Some("canceled")))
        }))
    }
}
