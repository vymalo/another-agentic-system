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
//! | `messages` | `working`, two agent `Message` frames (`message one`, `message two`, ids `<task>-msg-<n>`), artifact `echo: <text>`, `completed`; when the text also contains the word `gate`, it waits for [`FakeAgent::release_gate`] after the messages |
//! | `auth` | `working`, `auth-required("github")`; the follow-up on the same task behaves like the one of `ask` |
//! | `ui` | `working`, two artifacts that are only A2UI parts (surface `s1`: a `createSurface`, then an `updateComponents` with a button), `input-required("Pick one")`; the follow-up (an A2UI action, or text) answers like `ask` |
//! | `ui-msg` | `working`, an agent `Message` with text and an A2UI part, artifact, `completed` |
//! | `ui-status` | `working`, then `input-required` whose message holds text and an A2UI part (a form in the question) |
//! | `ui-bad` | `working`, an artifact whose A2UI part is an object, not an array, then `completed` |
//! | `ui-big` | `working`, an artifact whose A2UI part is larger than the cap, then `completed` |
//! | `ui-delete` | `working`, an A2UI part that creates surface `s2`, then one that deletes it, `completed` |
//! | `verify-pass` | `working`, artifacts `branch` (a commit) and `checks` (`passed: true`), `completed` |
//! | `verify-red-once` | as `verify-pass`, but `checks` fails (with a finding) until the message is the rework prompt of attempt 2 or later; then it passes |
//! | `verify-red` | as `verify-pass`, but `checks` always fails |
//! | `verify-ci` | `working`, only the `branch` artifact (a commit named by [`verify_commit`] in [`VERIFY_REPOSITORY`]), `completed`: an agent that pushed and leaves the checking to CI |
//!
//! A rework prompt (the message the gate sends an agent whose work failed, ADR 0018: it starts
//! with "Your work did not pass verification" and says "this is attempt N") is answered as the
//! script the context started with, at attempt N, and is a **new task** of the same context: the
//! commit is `<N as 40 hex digits>`, so each attempt pushes its own.
//!
//! An action arrives as a data part of `application/a2ui+json`; its name becomes the text the
//! script sees (`ui-action <name>`), and [`Call::actions`] records the messages.
//!
//! With [`FakeAgentOptions::ui_extensions`] the card lists the A2UI extension under those URIs,
//! and [`FakeAgent::set_ui_extensions`] changes them while the agent runs (the card is produced
//! on every request). Each call records the renderer capabilities the message carried.
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
    ServiceParams,
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

/// The media type of an A2UI part (kept literal: test support must not depend on the adapter).
pub const A2UI_MEDIA_TYPE: &str = "application/a2ui+json";

/// The URL every finished script reports as its artifact.
pub const PR_URL: &str = "https://github.com/acme/demo/pull/1";

/// The repository the `verify-*` scripts report in their `branch` artifact.
pub const VERIFY_REPOSITORY: &str = "https://github.com/acme/demo.git";

/// The commit the `verify-*` scripts report at `attempt` (1 for the first delegation).
pub fn verify_commit(attempt: u32) -> String {
    format!("{attempt:040x}")
}

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
    /// URIs the card lists as A2UI extensions (empty: the card does not mention A2UI).
    pub ui_extensions: Vec<String>,
}

impl Default for FakeAgentOptions {
    fn default() -> Self {
        FakeAgentOptions {
            bearer: None,
            releases: None,
            resubscribe: true,
            bind: None,
            ui_extensions: Vec::new(),
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
    /// The renderer capabilities of the message: the value of `a2uiClientCapabilities` or
    /// `a2uiRendererCapabilities` in its metadata, when it carried one.
    pub a2ui_capabilities: Option<Value>,
    /// The A2UI action messages the message carried (the array elements of its A2UI data parts).
    pub actions: Vec<Value>,
}

impl Call {
    /// The request activated the A2UI extension under `uri`.
    pub fn activates(&self, uri: &str) -> bool {
        self.extensions_header
            .iter()
            .any(|h| h.split(',').any(|e| e.trim() == uri))
    }

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
    /// The A2UI extension URIs the card lists right now.
    ui_extensions: Mutex<Vec<String>>,
    /// The `verify-*` script each context started with, so that its rework prompts (which do
    /// not repeat the word) run the same one.
    verifying: Mutex<HashMap<String, String>>,
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
            ui_extensions: Mutex::new(opts.ui_extensions.clone()),
            verifying: Mutex::new(HashMap::new()),
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
                    LiveCard {
                        base: card,
                        shared: Arc::clone(&shared),
                    },
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
        AgentEndpoint::a2a(AgentId::new(id), self.card_url(), bearer.map(str::to_owned))
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

    /// Changes which A2UI extension URIs the card lists, from the next card request on.
    pub fn set_ui_extensions(&self, uris: &[&str]) {
        *lock(&self.shared.ui_extensions) = uris.iter().map(|u| (*u).to_owned()).collect();
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

/// The card, with the A2UI extensions the agent lists at the moment of the request.
struct LiveCard {
    base: AgentCard,
    shared: Arc<Shared>,
}

impl a2a_server::agent_card::AgentCardProducer for LiveCard {
    fn card(&self) -> AgentCard {
        let mut card = self.base.clone();
        let uris = lock(&self.shared.ui_extensions).clone();
        let extensions = card.capabilities.extensions.get_or_insert_with(Vec::new);
        extensions.extend(uris.into_iter().map(|uri| AgentExtension {
            uri,
            description: Some("Ability to render A2UI".to_owned()),
            required: Some(false),
            params: Some(HashMap::from([(
                "supportedCatalogIds".to_owned(),
                json!(["https://a2ui.org/specification/v0_9_1/catalogs/basic/catalog.json"]),
            )])),
        }));
        if card
            .capabilities
            .extensions
            .as_ref()
            .is_some_and(Vec::is_empty)
        {
            card.capabilities.extensions = None;
        }
        card
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

    /// A status whose message holds `text` and an A2UI part.
    fn status_with_ui(&self, state: TaskState, text: &str, ops: Value) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: self.task_id.clone(),
            context_id: self.context_id.clone(),
            status: TaskStatus {
                state,
                message: Some({
                    let mut m = Message::new(Role::Agent, vec![Part::text(text), ui_part(ops)]);
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

    /// An agent `Message` frame of the task, with a fixed id (a replay carries the same one).
    fn message(&self, n: u32, text: &str) -> StreamResponse {
        let mut m = Message::new(Role::Agent, vec![Part::text(text)]);
        m.message_id = format!("{}-msg-{n}", self.task_id);
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

fn is_a2ui(part: &Part) -> bool {
    part.media_type.as_deref() == Some(A2UI_MEDIA_TYPE)
        || part
            .metadata
            .as_ref()
            .and_then(|m| m.get("mimeType"))
            .and_then(Value::as_str)
            == Some(A2UI_MEDIA_TYPE)
}

/// The messages of the A2UI data parts of `message` (the array elements).
fn a2ui_messages(message: Option<&Message>) -> Vec<Value> {
    message
        .into_iter()
        .flat_map(|m| m.parts.iter())
        .filter(|p| is_a2ui(p))
        .filter_map(|p| match &p.content {
            a2a::PartContent::Data(Value::Array(items)) => Some(items.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The user's text; for a message that is an A2UI action, `ui-action <name>`, so scripts can
/// tell what was done.
fn text_of(message: Option<&Message>) -> String {
    let text = message
        .map(|m| {
            m.parts
                .iter()
                .filter_map(Part::as_text)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !text.is_empty() {
        return text;
    }
    a2ui_messages(message)
        .iter()
        .find_map(|m| m.get("action")?.get("name")?.as_str())
        .map(|name| format!("ui-action {name}"))
        .unwrap_or_default()
}

fn capabilities_of(message: Option<&Message>) -> Option<Value> {
    let metadata = message?.metadata.as_ref()?;
    metadata
        .get("a2uiClientCapabilities")
        .or_else(|| metadata.get("a2uiRendererCapabilities"))
        .cloned()
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
            a2ui_capabilities: capabilities_of(ctx.message.as_ref()),
            actions: a2ui_messages(ctx.message.as_ref()),
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

/// A data part of A2UI messages, spelled as the A2UI extension does (`metadata.mimeType`) and as
/// A2A 1.0 does (`mediaType`).
fn ui_part(ops: Value) -> Part {
    let mut part = Part::data(ops).with_media_type(A2UI_MEDIA_TYPE);
    part.metadata = Some(HashMap::from([(
        "mimeType".to_owned(),
        json!(A2UI_MEDIA_TYPE),
    )]));
    part
}

fn surface_ops(surface: &str) -> Value {
    json!([
        {"version": "v0.9.1", "createSurface": {
            "surfaceId": surface,
            "catalogId": "https://a2ui.org/specification/v0_9_1/catalogs/basic/catalog.json"}},
        {"version": "v0.9.1", "updateComponents": {"surfaceId": surface, "components": [
            {"id": "root", "component": "Column", "children": ["title", "go"]},
            {"id": "title", "component": "Text", "text": "Pick one"},
            {"id": "go_label", "component": "Text", "text": "Go"},
            {"id": "go", "component": "Button", "child": "go_label", "variant": "primary",
             "action": {"event": {"name": "go", "context": {"choice": "a"}}}}
        ]}}
    ])
}

/// How a rework prompt of the gate starts (`orch_core` writes it; kept literal like the rest of
/// this file, so test support does not depend on the wording being imported).
const REWORK_PREFIX: &str = "Your work did not pass verification";

/// The attempt a rework prompt says it is ("this is attempt 2"); 1 for any other message.
fn attempt_of(text: &str) -> u32 {
    text.split_once("this is attempt ")
        .and_then(|(_, rest)| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .unwrap_or(1)
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
    let reworking = !answering && text.starts_with(REWORK_PREFIX);
    let word = if answering {
        "ask".to_owned()
    } else if reworking {
        // The gate sent the agent back: run the script this context started with.
        lock(&shared.verifying)
            .get(&ctx.context_id)
            .cloned()
            .unwrap_or_default()
    } else {
        text.split_whitespace().next().unwrap_or("").to_owned()
    };
    let word = word.as_str();
    if word.starts_with("verify-") && !reworking {
        lock(&shared.verifying).insert(ctx.context_id.clone(), word.to_owned());
    }
    let finish = |id: String, body: String| {
        let artifact = ctx.artifact(&id, "result", echo_parts(&body), false, None);
        (artifact, ctx.status(TaskState::Completed, None))
    };
    match word {
        "fail" => emit(&tx, ctx.status(TaskState::Failed, Some("scripted failure"))).await?,
        "verify-pass" | "verify-red-once" | "verify-red" | "verify-ci" => {
            let attempt = if reworking { attempt_of(&text) } else { 1 };
            let passes = match word {
                "verify-pass" => true,
                "verify-red-once" => attempt >= 2,
                _ => false,
            };
            let commit = verify_commit(attempt);
            let branch = json!({
                "repository": VERIFY_REPOSITORY,
                "branch": "agent/fix",
                "commit": commit,
            });
            let checks = if passes {
                json!({"passed": true, "commit": commit, "summary": "3 tests passed"})
            } else {
                json!({
                    "passed": false,
                    "commit": commit,
                    "summary": "1 test failed",
                    "findings": ["tests::login fails: expected 200, got 500"],
                })
            };
            let reported = if word == "verify-ci" {
                vec![("branch", branch)]
            } else {
                vec![("branch", branch), ("checks", checks)]
            };
            for (name, data) in reported {
                let id = shared.next_artifact_id();
                emit(
                    &tx,
                    ctx.artifact(&id, name, vec![Part::data(data)], false, Some(true)),
                )
                .await?;
            }
            emit(&tx, ctx.status(TaskState::Completed, None)).await?;
        }
        "auth" if !answering => {
            lock(&shared.asking).insert(ctx.task_id.clone());
            emit(&tx, ctx.status(TaskState::AuthRequired, Some("github"))).await?;
        }
        "messages" => {
            emit(&tx, ctx.message(1, "message one")).await?;
            emit(&tx, ctx.message(2, "message two")).await?;
            if text.split_whitespace().any(|w| w == "gate") {
                shared.gate.notified().await;
            }
            let (a, done) = finish(shared.next_artifact_id(), format!("echo: {text}"));
            emit(&tx, a).await?;
            emit(&tx, done).await?;
        }
        "ui" if !answering => {
            lock(&shared.asking).insert(ctx.task_id.clone());
            let ops = surface_ops("s1");
            let ops = ops.as_array().cloned().unwrap_or_default();
            for op in ops {
                let id = shared.next_artifact_id();
                emit(
                    &tx,
                    ctx.artifact(
                        &id,
                        "form",
                        vec![ui_part(Value::Array(vec![op]))],
                        false,
                        Some(true),
                    ),
                )
                .await?;
            }
            emit(&tx, ctx.status(TaskState::InputRequired, Some("Pick one"))).await?;
        }
        "ui-msg" => {
            let mut m = Message::new(
                Role::Agent,
                vec![Part::text("Here is a form"), ui_part(surface_ops("s1"))],
            );
            m.message_id = format!("{}-ui-msg", ctx.task_id);
            m.task_id = Some(ctx.task_id.clone());
            m.context_id = Some(ctx.context_id.clone());
            m.metadata = ctx.metadata.clone();
            emit(&tx, StreamResponse::Message(m)).await?;
            let (a, done) = finish(shared.next_artifact_id(), format!("echo: {text}"));
            emit(&tx, a).await?;
            emit(&tx, done).await?;
        }
        "ui-status" if !answering => {
            lock(&shared.asking).insert(ctx.task_id.clone());
            emit(
                &tx,
                ctx.status_with_ui(TaskState::InputRequired, "Which one?", surface_ops("s1")),
            )
            .await?;
        }
        "ui-bad" | "ui-big" => {
            let payload = if word == "ui-bad" {
                json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s1"}})
            } else {
                json!([{"version": "v0.9.1", "updateDataModel": {
                    "surfaceId": "s1", "value": "x".repeat(70 * 1024)}}])
            };
            let id = shared.next_artifact_id();
            emit(
                &tx,
                ctx.artifact(&id, "form", vec![ui_part(payload)], false, Some(true)),
            )
            .await?;
            emit(&tx, ctx.status(TaskState::Completed, None)).await?;
        }
        "ui-delete" => {
            let create = json!([{"version": "v0.9.1", "createSurface": {"surfaceId": "s2"}}]);
            let delete = json!([{"version": "v0.9.1", "deleteSurface": {"surfaceId": "s2"}}]);
            for ops in [create, delete] {
                let id = shared.next_artifact_id();
                emit(
                    &tx,
                    ctx.artifact(&id, "form", vec![ui_part(ops)], false, Some(true)),
                )
                .await?;
            }
            emit(&tx, ctx.status(TaskState::Completed, None)).await?;
        }
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
        let resuming = ctx.stored_task.as_ref().is_some_and(|t| {
            matches!(
                t.status.state,
                TaskState::InputRequired | TaskState::AuthRequired
            )
        });
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
