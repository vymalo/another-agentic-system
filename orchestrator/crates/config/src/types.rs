//! The keys of the configuration file ([`docs/api/config.md`](../../../../docs/api/config.md)).
//!
//! Every struct is closed (`deny_unknown_fields`, so the schema says `additionalProperties:
//! false`), camel-cased, and free of `#[serde(flatten)]` (serde does not support it together with
//! `deny_unknown_fields`). The doc comment of a key is its description in the generated JSON
//! Schema, so an editor shows it. A secret is a [`SecretRef`], never a value.
//!
//! The ranges and defaults here are the schema's too: the shape pass reports a value out of range
//! with its key path, and the rules pass checks what a range cannot say.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The default of `server.listen`.
pub const DEFAULT_LISTEN: &str = "0.0.0.0:8080";

/// A reference to a secret: where its value is read from at startup, never the value.
///
/// `{ env: NAME }` reads the environment variable `NAME` (it must be set and not empty);
/// `{ file: PATH }` reads the file at `PATH` (a relative path is relative to the directory of the
/// configuration file; one trailing newline is cut; at most 64 KiB). A plain string where a secret
/// goes is an error.
#[derive(Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SecretRef {
    /// The name of an environment variable that holds the secret.
    Env(String),
    /// The path of a file that holds the secret.
    File(String),
}

impl Serialize for SecretRef {
    /// As the one-entry mapping it is written as in the file, `{ env: NAME }`, in JSON and in
    /// YAML alike (the derived form would be a YAML tag).
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            SecretRef::Env(name) => map.serialize_entry("env", name)?,
            SecretRef::File(path) => map.serialize_entry("file", path)?,
        }
        map.end()
    }
}

impl std::fmt::Debug for SecretRef {
    /// A reference is not a secret: it prints as written in the file.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretRef::Env(name) => write!(f, "{{ env: {name} }}"),
            SecretRef::File(path) => write!(f, "{{ file: {path} }}"),
        }
    }
}

/// A URL that is written in the file, or read from a secret reference when a deployment keeps it
/// out of git (the gateway's address, next to its key).
///
/// A plain string is the URL itself, as it always was. `{ env: NAME }` and `{ file: PATH }` read
/// it at startup the way a [`SecretRef`] does (same trimming, same 64 KiB limit); the value read
/// is then held to the same rule a written URL is. A URL is not a secret: `--print-config` shows
/// the reference and the value is never logged as if it were public, but it is not redacted
/// as a credential either.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum UrlRef {
    /// The URL, as written in the file.
    Literal(String),
    /// Where the URL is read from at startup.
    Ref(SecretRef),
}

impl std::fmt::Debug for UrlRef {
    /// As written in the file: the URL or the reference.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlRef::Literal(url) => write!(f, "{url:?}"),
            UrlRef::Ref(reference) => write!(f, "{reference:?}"),
        }
    }
}

/// The configuration of the orchestrator (`version: 1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "Orchestrator configuration")]
pub struct Config {
    /// The version of this file's format. This build reads `1`.
    #[schemars(range(min = 1, max = 1))]
    pub version: u32,
    /// The process: the address it listens on, its role, the interaction surfaces it mounts.
    #[serde(default)]
    pub server: Server,
    /// Logging.
    #[serde(default)]
    pub log: Log,
    /// The Postgres database, the one place the system keeps state.
    pub database: Database,
    /// The dispatcher: the outbox rows worked at once, and the lease on them.
    #[serde(default)]
    pub dispatcher: Dispatcher,
    /// The inbox worker: timers and stored reports.
    #[serde(default)]
    pub inbox: Inbox,
    /// The agents: a file, in-process agents, the platform's registry.
    #[serde(default)]
    pub agents: Agents,
    /// The deployment's layer of the verification gate (ADR 0018).
    #[serde(default)]
    pub gate: Gate,
    /// What a step records.
    #[serde(default)]
    pub steps: Steps,
    /// Asked agents: how deep, how many and how long an agent may ask the agents the person
    /// mentioned (ADR 0026, the thread tool `ask_agent`).
    #[serde(default)]
    pub asks: Asks,
    /// The OpenAI-compatible endpoints the orchestrator asks itself (ADR 0005, ADR 0035).
    #[serde(default)]
    pub models: Models,
    /// The utility tasks that use a model: a task that is absent is off (ADR 0035).
    #[serde(default)]
    pub tasks: Tasks,
    /// The artifact store: where the files agents hand over are kept (ADR 0032). Absent: no store,
    /// and a file an agent hands over is refused with "no artifact store configured".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Artifacts>,
    /// What the web is told (`GET /api/config`).
    #[serde(default)]
    pub ui: Ui,
    /// The thread tools: the endpoint agents call back, and the key of its tokens.
    #[serde(default)]
    pub thread_tools: ThreadTools,
    /// Sharing a thread by a revocable link (ADR 0040): the cap on what a deployment allows, the
    /// secret the links are made with, what a public reader may see. Absent: nothing can be
    /// shared (`mode: disabled`).
    #[serde(default)]
    pub sharing: Sharing,
    /// The MCP server surface (ADR 0019).
    #[serde(default)]
    pub mcp: Mcp,
    /// The CI webhook surfaces (ADR 0017).
    #[serde(default)]
    pub webhooks: Webhooks,
    /// Authentication.
    #[serde(default)]
    pub auth: Auth,
    /// The MCP servers a person may attach to a conversation (ADR 0024), in the order the web
    /// lists them. Absent or empty: nothing is attachable. At most 64.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = 64))]
    pub tool_servers: Vec<ToolServer>,
}

/// What this process runs. A closed enum owned by the host library (`adam-host`); the names are
/// the same for every host. `all`: the control plane and the workers, in one process.
/// `control-plane`: the HTTP server, the resource API and the surfaces, no dispatcher. `worker`:
/// the dispatcher, and a router with only `/healthz` and `/readyz`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// The control plane and the workers, in one process.
    #[default]
    All,
    /// The HTTP server, the resource API and the surfaces; no dispatcher.
    ControlPlane,
    /// The dispatcher, and a router with only `/healthz` and `/readyz`.
    Worker,
}

impl Role {
    /// The name used in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::All => "all",
            Role::ControlPlane => "control-plane",
            Role::Worker => "worker",
        }
    }

    /// Whether this process serves the HTTP routes (and so mounts the surfaces).
    pub const fn runs_control_plane(self) -> bool {
        matches!(self, Role::All | Role::ControlPlane)
    }
}

/// An interaction surface: a set of HTTP routes the orchestrator can mount. `agui`: the AG-UI
/// routes the web speaks (ADR 0012). `mcp`: the MCP server at `/mcp` (ADR 0019; needs
/// `mcp.tokensFile` and `mcp.allowedHosts`). `thread-tools`: the per-thread MCP endpoint
/// (`thread-tools/v1`; needs `threadTools.url` and `secret`). `webhook-generic`: `POST
/// /webhooks/ci`, the generic signed CI report (needs `webhooks.generic`). `webhook-github`:
/// `POST /webhooks/github`, GitHub's own deliveries (needs `webhooks.github`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    /// The AG-UI routes the web speaks (ADR 0012).
    Agui,
    /// The MCP server at `/mcp` (ADR 0019). Needs `mcp.tokensFile` and `mcp.allowedHosts`.
    Mcp,
    /// The per-thread MCP endpoint (`thread-tools/v1`). Needs `threadTools.url` and `secret`.
    ThreadTools,
    /// `POST /webhooks/ci`, the generic signed CI report. Needs `webhooks.generic`.
    WebhookGeneric,
    /// `POST /webhooks/github`, GitHub's own deliveries. Needs `webhooks.github`.
    WebhookGithub,
}

impl Surface {
    /// The name used in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Surface::Agui => "agui",
            Surface::Mcp => "mcp",
            Surface::ThreadTools => "thread-tools",
            Surface::WebhookGeneric => "webhook-generic",
            Surface::WebhookGithub => "webhook-github",
        }
    }
}

/// The process: where it listens, what it runs, what it mounts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Server {
    /// The socket address to listen on (default `0.0.0.0:8080`). Replaces `LISTEN_ADDR`.
    #[serde(default = "default_listen")]
    pub listen: String,
    /// What this process runs (default `all`). `ORCH_ROLE` and `--role` stay as a process
    /// override: a control plane and its workers share one file.
    #[serde(default)]
    pub role: Role,
    /// Names this replica in outbox leases (default `$HOSTNAME-<uuid>`). `ORCH_INSTANCE_ID` and
    /// `--instance-id` stay as a process override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub instance_id: Option<String>,
    /// The surfaces to mount, as far as the build has them (default `[agui]`). Replaces
    /// `ORCH_SURFACES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub surfaces: Option<Vec<Surface>>,
    /// The public origin of the chat, for example `https://chat.example.com`: the MCP surface
    /// gives `start_job` a `web_url` from it. Replaces `ORCH_PUBLIC_URL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// Seconds a graceful shutdown may take (default 15). Replaces `SHUTDOWN_GRACE_SECS`.
    #[serde(default = "default_shutdown_grace_secs")]
    #[schemars(range(min = 1))]
    pub shutdown_grace_secs: u64,
    /// `development` (default) or `production`. A `production` process refuses
    /// `auth.mode: proxy_header` (and so `auth.devUser`): the identity header is only as good as
    /// the proxy that strips it (ADR 0033).
    #[serde(default)]
    pub environment: Environment,
    /// Calls from a page of another origin (CORS, ADR 0047): the desktop and mobile apps, or a web
    /// served elsewhere. Absent: no CORS header is sent, and a browser refuses every cross-origin
    /// call, which is what the web on the edge's own origin needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cors: Option<ServerCors>,
    /// What one page of a thread's history may hold (ADR 0059, `docs/api/history.md`).
    #[serde(default)]
    pub history: ServerHistory,
}

/// The bounds of a page of a thread's history (`server.history`, ADR 0059): the route
/// `GET /agui/threads/{threadId}/history` and its two shared variants. The newest chain of a page is
/// returned whole whatever its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerHistory {
    /// The most turns a page holds, 1 to 1000 (default 100): the largest `limit` a request may ask
    /// for and the most a `since` read goes back.
    #[serde(default = "default_history_max_turns")]
    #[schemars(range(min = 1, max = 1000))]
    pub max_turns: u32,
    /// The most bytes of frames a page holds, as JSON, 64 KiB to 64 MiB (default 4 MiB): chains
    /// older than the newest that do not fit are left out.
    #[serde(default = "default_history_max_page_bytes")]
    #[schemars(range(min = 65_536, max = 67_108_864))]
    pub max_page_bytes: u64,
}

fn default_history_max_turns() -> u32 {
    100
}

fn default_history_max_page_bytes() -> u64 {
    4 * 1024 * 1024
}

impl Default for ServerHistory {
    fn default() -> Self {
        ServerHistory {
            max_turns: default_history_max_turns(),
            max_page_bytes: default_history_max_page_bytes(),
        }
    }
}

/// The origins that may call this API from a page of their own (`server.cors`, ADR 0047). The answer
/// allows the methods of the API, the request headers `Authorization`, `DPoP`, `Content-Type`,
/// `Accept`, `Last-Event-ID` and `X-Web-Revision`, exposes `WWW-Authenticate`, `Date` and
/// `Content-Disposition`, and **never credentials**: a cross-origin caller sends a DPoP-bound token,
/// never a cookie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerCors {
    /// Each an origin compared exactly with the request's `Origin`: `scheme://host[:port]`, with no
    /// path, query, fragment, credentials or trailing slash, for example `tauri://localhost` or
    /// `http://tauri.localhost`. Never `*` or `null`. In production `http://` is refused, but for
    /// `localhost`, `127.0.0.1`, `[::1]` and a name under `.localhost` (Tauri's own on Windows and
    /// Android).
    #[schemars(length(min = 1))]
    pub allowed_origins: Vec<String>,
}

/// Where this process runs. `production` makes the configuration refuse what is only for a
/// single user on a local machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Environment {
    /// A local or shared development stack.
    #[default]
    Development,
    /// A deployment that serves real people.
    Production,
}

fn default_listen() -> String {
    DEFAULT_LISTEN.to_owned()
}

fn default_shutdown_grace_secs() -> u64 {
    15
}

impl Default for Server {
    fn default() -> Self {
        Server {
            listen: default_listen(),
            role: Role::default(),
            instance_id: None,
            surfaces: None,
            public_url: None,
            shutdown_grace_secs: default_shutdown_grace_secs(),
            environment: Environment::default(),
            cors: None,
            history: ServerHistory::default(),
        }
    }
}

/// The format of a log line: `json`, one object per line, what a log collector wants; or `text`,
/// human-readable lines for local development.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LogFormat {
    /// One JSON object per line: what a log collector wants.
    #[default]
    Json,
    /// Human-readable lines for local development.
    Text,
}

impl LogFormat {
    /// The name used in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            LogFormat::Json => "json",
            LogFormat::Text => "text",
        }
    }
}

/// Logging. The filter is `RUST_LOG`, read before anything else (default `info,rmcp=warn`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Log {
    /// The format of a log line (default `json`). Replaces `LOG_FORMAT`.
    #[serde(default)]
    pub format: LogFormat,
}

/// The Postgres database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Database {
    /// The connection string, a secret. Replaces `DATABASE_URL`.
    pub url: SecretRef,
    /// The pool size, at least 2: the wakeup listener holds one (default 10). Replaces
    /// `DATABASE_MAX_CONNECTIONS`.
    #[serde(default = "default_max_connections")]
    #[schemars(range(min = 2))]
    pub max_connections: u64,
}

fn default_max_connections() -> u64 {
    10
}

/// The dispatcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dispatcher {
    /// Outbox rows processed at once (default 32). Replaces `DISPATCHER_CONCURRENCY`.
    #[serde(default = "default_concurrency")]
    #[schemars(range(min = 1))]
    pub concurrency: u64,
    /// Seconds a crashed replica's claim blocks others, at least 3 (default 30). Also how long a
    /// local agent's run stays leased. Replaces `OUTBOX_LEASE_SECS`.
    #[serde(default = "default_lease_secs")]
    #[schemars(range(min = 3))]
    pub outbox_lease_secs: u64,
}

fn default_concurrency() -> u64 {
    32
}

fn default_lease_secs() -> u64 {
    30
}

impl Default for Dispatcher {
    fn default() -> Self {
        Dispatcher {
            concurrency: default_concurrency(),
            outbox_lease_secs: default_lease_secs(),
        }
    }
}

/// The inbox worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inbox {
    /// Seconds a worker's claim on a row lasts, at least 3 (default 30). Replaces
    /// `INBOX_LEASE_SECS`.
    #[serde(default = "default_lease_secs")]
    #[schemars(range(min = 3))]
    pub lease_secs: u64,
    /// Seconds between the worker's polls, at least 1 (default 2). Replaces `INBOX_POLL_SECS`.
    #[serde(default = "default_poll_secs")]
    #[schemars(range(min = 1))]
    pub poll_secs: u64,
    /// Seconds a report that matches no watch waits before it expires, at least 1 (default
    /// 86400). Replaces `INBOX_PARKED_TTL_SECS`.
    #[serde(default = "default_parked_ttl_secs")]
    #[schemars(range(min = 1))]
    pub parked_ttl_secs: u64,
    /// Claims of one row before it is given up on, at least 1 (default 10). Replaces
    /// `INBOX_MAX_ATTEMPTS`.
    #[serde(default = "default_inbox_max_attempts")]
    #[schemars(range(min = 1))]
    pub max_attempts: u64,
}

fn default_poll_secs() -> u64 {
    2
}

fn default_parked_ttl_secs() -> u64 {
    86_400
}

fn default_inbox_max_attempts() -> u64 {
    10
}

impl Default for Inbox {
    fn default() -> Self {
        Inbox {
            lease_secs: default_lease_secs(),
            poll_secs: default_poll_secs(),
            parked_ttl_secs: default_parked_ttl_secs(),
            max_attempts: default_inbox_max_attempts(),
        }
    }
}

/// The agents of this deployment.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Agents {
    /// The agents file: a YAML list of `{id, name, transport?, cardUrl?, tokenEnv?, agent?,
    /// gate?}`, relative to this file's directory. Required unless `registry.url` is set.
    /// Replaces `AGENTS_FILE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub file: Option<String>,
    /// Runs of local agents stepped at once, at least 1 (default 4). A build without the Cargo
    /// feature `agent-local` refuses the key. Replaces `AGENT_LOCAL_CONCURRENCY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub local_concurrency: Option<u64>,
    /// The platform's agent registry (`agent-registry/v1`, ADR 0022). A build without the Cargo
    /// feature `registry-platform` refuses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<Registry>,
}

/// The platform's agent registry, read live beside the agents file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registry {
    /// The full URL of the document, `http` or `https`, with a host and no user name or
    /// password. Replaces `AGENT_REGISTRY_URL`.
    pub url: String,
    /// The bearer token sent to the registry, when it wants one. Replaces
    /// `AGENT_REGISTRY_TOKEN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<SecretRef>,
    /// The bearer token sent to every agent the registry lists. Replaces
    /// `AGENT_REGISTRY_AGENT_TOKEN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_token: Option<SecretRef>,
    /// Seconds one read may take, 1 to 60 (default 3). Replaces `AGENT_REGISTRY_TIMEOUT_SECS`.
    #[serde(default = "default_registry_timeout_secs")]
    #[schemars(range(min = 1, max = 60))]
    pub timeout_secs: u64,
    /// The most seconds a copy of the document is kept, 1 to 3600 (default 60). Replaces
    /// `AGENT_REGISTRY_MAX_AGE_SECS`.
    #[serde(default = "default_registry_max_age_secs")]
    #[schemars(range(min = 1, max = 3600))]
    pub max_age_secs: u64,
}

fn default_registry_timeout_secs() -> u64 {
    3
}

fn default_registry_max_age_secs() -> u64 {
    60
}

/// A source a job's work can be required to pass (ADR 0018). `ci`: CI reports the pushed commit
/// green (needs `gate.ci.required` and a CI webhook surface). `agent-checks`: the agent's own
/// `checks` artifact passes. `verifier`: another agent reviews the pushed commit (needs
/// `gate.verifier`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum GateSource {
    /// CI reports the pushed commit green. Needs `gate.ci.required` and a CI webhook surface.
    Ci,
    /// The agent's own `checks` artifact passes.
    AgentChecks,
    /// Another agent reviews the pushed commit. Needs `gate.verifier`.
    Verifier,
}

impl GateSource {
    /// The name used in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            GateSource::Ci => "ci",
            GateSource::AgentChecks => "agent-checks",
            GateSource::Verifier => "verifier",
        }
    }
}

/// The deployment's layer of the verification gate. The members have the names of an agent
/// entry's `gate` in the agents file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Gate {
    /// The sources every job's work must pass before it is done (default none: an agent that
    /// completes is done). Replaces `ORCH_GATE`.
    #[serde(default)]
    pub require: Vec<GateSource>,
    /// Attempts a job's agent gets under a gate, the first included, at least 1 and at most
    /// `maxAttemptsCap` (default 3, lowered to the cap when the cap is lower). Replaces
    /// `ORCH_MAX_ATTEMPTS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_attempts: Option<u64>,
    /// The most a target or a thread may raise the attempts to, 1 to 100 (default 10). Replaces
    /// `ORCH_MAX_ATTEMPTS_CAP`.
    #[serde(default = "default_max_attempts_cap")]
    #[schemars(range(min = 1, max = 100))]
    pub max_attempts_cap: u64,
    /// The agent that verifies the pushed work: the id of another configured agent. Needed when
    /// the gate requires `verifier`. Replaces `ORCH_VERIFIER`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub verifier: Option<String>,
    /// Seconds the verifier has to answer before the thread waits for the user, at least 1
    /// (default 1800). Replaces `ORCH_VERIFIER_TIMEOUT_SECS`.
    #[serde(default = "default_verifier_timeout_secs")]
    #[schemars(range(min = 1))]
    pub verifier_timeout_secs: u64,
    /// Seconds between a verification's looks at its thread, at least 1 (default 5). Replaces
    /// `ORCH_VERIFIER_WATCH_SECS`.
    #[serde(default = "default_verifier_watch_secs")]
    #[schemars(range(min = 1))]
    pub verifier_watch_secs: u64,
    /// The CI checks of the deployment's gate.
    #[serde(default)]
    pub ci: GateCi,
}

fn default_max_attempts_cap() -> u64 {
    10
}

fn default_verifier_timeout_secs() -> u64 {
    1800
}

fn default_verifier_watch_secs() -> u64 {
    5
}

impl Default for Gate {
    fn default() -> Self {
        Gate {
            require: Vec::new(),
            max_attempts: None,
            max_attempts_cap: default_max_attempts_cap(),
            verifier: None,
            verifier_timeout_secs: default_verifier_timeout_secs(),
            verifier_watch_secs: default_verifier_watch_secs(),
            ci: GateCi::default(),
        }
    }
}

/// The CI settings of the deployment's gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GateCi {
    /// The names of the CI checks that must pass: GitHub's check name or workflow name, or the
    /// `name` of a generic report. A gate that requires `ci` must name at least one, here or in
    /// an agent's `gate.ci.required`. Replaces `ORCH_CI_REQUIRED`.
    #[serde(default)]
    pub required: Vec<String>,
    /// Seconds a job waits for the CI reports its gate needs before it is blocked, at least 1
    /// (default 3600). Replaces `ORCH_CI_TIMEOUT_SECS`.
    #[serde(default = "default_ci_timeout_secs")]
    #[schemars(range(min = 1))]
    pub timeout_secs: u64,
}

fn default_ci_timeout_secs() -> u64 {
    3600
}

impl Default for GateCi {
    fn default() -> Self {
        GateCi {
            required: Vec::new(),
            timeout_secs: default_ci_timeout_secs(),
        }
    }
}

/// What a step records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Steps {
    /// Whether a step's input and output (what a tool was called with and what it returned) are
    /// recorded in the log, redacted and capped (ADR 0030; default `true`). Replaces
    /// `ORCH_STEPS_RECORD_IO`.
    #[serde(default = "default_true")]
    pub record_tool_io: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Steps {
    fn default() -> Self {
        Steps {
            record_tool_io: true,
        }
    }
}

/// The limits on asked agents (ADR 0026, `docs/api/thread-tools-v1.md` `ask_agent`): the owner's
/// decision 6 of plan 11 is the defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asks {
    /// How deep a chain of asks may go, 1 to 4 (default 2): the addressed agent asks A (depth 1),
    /// A may ask B (depth 2), and B cannot ask. Replaces
    /// `ORCH_ASK_MAX_DEPTH`.
    #[serde(default = "default_ask_depth")]
    #[schemars(range(min = 1, max = 4))]
    pub max_depth: u64,
    /// How many asks one job may make, those that ended included, 1 to 64 (default 16). Replaces
    /// `ORCH_ASK_MAX_PER_JOB`.
    #[serde(default = "default_ask_per_job")]
    #[schemars(range(min = 1, max = 64))]
    pub max_per_job: u64,
    /// How many asks of a thread may run at once, 1 to 16 (default 4). Replaces
    /// `ORCH_ASK_MAX_RUNNING`.
    #[serde(default = "default_ask_running")]
    #[schemars(range(min = 1, max = 16))]
    pub max_running: u64,
    /// Seconds an ask may run before it ends `timed_out` and the asked agent is told to stop, 10
    /// to 7200 (default 1800). An agent may ask for less in a call, never for more. Replaces
    /// `ORCH_ASK_TIMEOUT_SECS`.
    #[serde(default = "default_ask_timeout_secs")]
    #[schemars(range(min = 10, max = 7200))]
    pub timeout_secs: u64,
}

fn default_ask_depth() -> u64 {
    2
}

fn default_ask_per_job() -> u64 {
    16
}

fn default_ask_running() -> u64 {
    4
}

fn default_ask_timeout_secs() -> u64 {
    1800
}

impl Default for Asks {
    fn default() -> Self {
        Asks {
            max_depth: default_ask_depth(),
            max_per_job: default_ask_per_job(),
            max_running: default_ask_running(),
            timeout_secs: default_ask_timeout_secs(),
        }
    }
}

/// The model endpoints.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Models {
    /// Endpoints by name (a slug: `a-z`, `0-9` and `-`, up to 32 characters). The legacy
    /// `ORCH_MODEL_*` variables are the endpoint named `default`.
    #[serde(default)]
    pub endpoints: BTreeMap<String, Endpoint>,
}

/// An OpenAI-compatible endpoint (ADR 0005).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    /// The base URL, up to and not including `/chat/completions`, `http` or `https`: written
    /// here, or read from `{ env: NAME }` or `{ file: PATH }` when it is kept in a secret store.
    /// Replaces `ORCH_MODEL_BASE_URL`.
    pub base_url: UrlRef,
    /// The bearer token the endpoint wants, when it wants one. Replaces `ORCH_MODEL_API_KEY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretRef>,
    /// Seconds one question may take, at least 1 (default 20). Replaces
    /// `ORCH_MODEL_TIMEOUT_SECS`.
    #[serde(default = "default_model_timeout_secs")]
    #[schemars(range(min = 1))]
    pub timeout_secs: u64,
}

fn default_model_timeout_secs() -> u64 {
    20
}

/// The utility tasks: what the orchestrator asks a model for on its own account, each with the
/// endpoint, model, prompt and language rule it runs with (ADR 0035). A task that is absent is
/// off.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tasks {
    /// The thread title. Absent: titles are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<TitleTask>,
    /// The thread description. Absent: threads have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<DescriptionTask>,
}

/// The title task: after the agent's first reply, the model is asked for a 3 to 6 word title.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TitleTask {
    /// The name of an endpoint in `models.endpoints`.
    #[schemars(length(min = 1))]
    pub endpoint: String,
    /// The model's name at the endpoint. Replaces `ORCH_TITLE_MODEL`.
    #[schemars(length(min = 1))]
    pub model: String,
    /// The guidance that says what to write and in what style, in place of the core's own; read
    /// once at startup. The core always adds the form of the answer, the data clause, the fence
    /// around the conversation and the language line, last, whatever this says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<Prompt>,
    /// Most tokens of answer, 1 to 256 (default 32).
    #[serde(default = "default_title_max_tokens")]
    #[schemars(range(min = 1, max = 256))]
    pub max_tokens: u32,
    /// The language of the title: the person's (`conversation`, the default) or a fixed one.
    #[serde(default)]
    pub language: Language,
}

fn default_title_max_tokens() -> u32 {
    32
}

/// The description task: when a job ends or pauses for the person, the model is asked for a
/// sentence or two on what the thread is about now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DescriptionTask {
    /// The name of an endpoint in `models.endpoints`.
    #[schemars(length(min = 1))]
    pub endpoint: String,
    /// The model's name at the endpoint.
    #[schemars(length(min = 1))]
    pub model: String,
    /// The guidance that says what to write and in what style, in place of the core's own; read
    /// once at startup. The core always adds the form of the answer, the data clause, the fence
    /// around the conversation and the language line, last, whatever this says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<Prompt>,
    /// Most tokens of answer, 1 to 1024 (default 160).
    #[serde(default = "default_description_max_tokens")]
    #[schemars(range(min = 1, max = 1024))]
    pub max_tokens: u32,
    /// The language of the description: the person's (`conversation`, the default) or a fixed one.
    #[serde(default)]
    pub language: Language,
    /// Where a description is cut, at a word: 40 to 500 characters (default 300).
    #[serde(default = "default_description_max_chars")]
    #[schemars(range(min = 40, max = 500))]
    pub max_chars: u32,
    /// When a new description is asked for.
    #[serde(default)]
    pub recompute: Recompute,
}

fn default_description_max_tokens() -> u32 {
    160
}

fn default_description_max_chars() -> u32 {
    300
}

/// When a new description is asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recompute {
    /// How many messages (of the person, and the agent's final words) the conversation has to have
    /// grown by since the last description before a new one is asked for, at least 1 (default 4).
    /// Fewer is no model call.
    #[serde(default = "default_min_new_messages")]
    #[schemars(range(min = 1))]
    pub min_new_messages: u32,
}

fn default_min_new_messages() -> u32 {
    4
}

impl Default for Recompute {
    fn default() -> Self {
        Recompute {
            min_new_messages: default_min_new_messages(),
        }
    }
}

/// A prompt, given inline or by file: `{ inline: TEXT }` or `{ file: PATH }` (a relative path is
/// relative to the directory of the configuration file). UTF-8, 1 to 4096 bytes once the space
/// around it is cut. A file is read once, at startup.
#[derive(Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Prompt {
    /// The text itself.
    Inline(String),
    /// The path of a file that holds the text.
    File(String),
}

impl Serialize for Prompt {
    /// As the one-entry mapping it is written as in the file, `{ inline: TEXT }` or
    /// `{ file: PATH }`, in JSON and in YAML alike (the derived form would be a YAML tag, which the
    /// loader refuses).
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            Prompt::Inline(text) => map.serialize_entry("inline", text)?,
            Prompt::File(path) => map.serialize_entry("file", path)?,
        }
        map.end()
    }
}

impl std::fmt::Debug for Prompt {
    /// The reference is printed as written in the file; the text of an inline prompt is not a
    /// secret, but it is long, so it is cut.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Prompt::Inline(text) => write!(f, "{{ inline: {} bytes }}", text.len()),
            Prompt::File(path) => write!(f, "{{ file: {path} }}"),
        }
    }
}

/// The language a task writes in: `conversation` is the language the person writes in, found in
/// their messages and checked on the answer; any other is fixed, named in the request and checked
/// against its own script. The set is the core's, closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// The language of the person's messages.
    #[default]
    Conversation,
    /// English.
    English,
    /// French.
    French,
    /// German.
    German,
    /// Spanish.
    Spanish,
    /// Portuguese.
    Portuguese,
    /// Italian.
    Italian,
    /// Chinese.
    Chinese,
    /// Japanese.
    Japanese,
    /// Korean.
    Korean,
    /// A language written in Cyrillic.
    Cyrillic,
    /// A language written in Arabic script.
    Arabic,
    /// Hebrew.
    Hebrew,
    /// Greek.
    Greek,
    /// A language written in Devanagari.
    Devanagari,
    /// Thai.
    Thai,
}

impl Language {
    /// The name used in the file, which is the name `orch_core` reads
    /// (`LanguageRule::from_config_name`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Language::Conversation => "conversation",
            Language::English => "english",
            Language::French => "french",
            Language::German => "german",
            Language::Spanish => "spanish",
            Language::Portuguese => "portuguese",
            Language::Italian => "italian",
            Language::Chinese => "chinese",
            Language::Japanese => "japanese",
            Language::Korean => "korean",
            Language::Cyrillic => "cyrillic",
            Language::Arabic => "arabic",
            Language::Hebrew => "hebrew",
            Language::Greek => "greek",
            Language::Devanagari => "devanagari",
            Language::Thai => "thai",
        }
    }
}

/// What the web is told about how to show things: the one part of the file it can read, served as
/// `GET /api/config` (ADR 0034). **It has no secret and never will**: a test asserts that its
/// schema holds no secret reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ui {
    /// Whether the web shows a thread's description (default true). The API returns it either
    /// way.
    #[serde(default = "default_true")]
    pub show_descriptions: bool,
    /// How the web opens a long thread (ADR 0059). `GET /api/config` serves it only when the
    /// process mounts the AG-UI surface, which is where the history route is, so its presence is
    /// the capability.
    #[serde(default)]
    pub history: UiHistory,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            show_descriptions: true,
            history: UiHistory::default(),
        }
    }
}

/// `ui.history`: what the web asks for when it opens a thread at its end (ADR 0059). Both counts
/// are at most `server.history.maxTurns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiHistory {
    /// The turns the web asks for when it opens a thread, 1 to 100 (default 12).
    #[serde(default = "default_initial_turns")]
    #[schemars(range(min = 1, max = 100))]
    pub initial_turns: u32,
    /// The turns of the first older page the web asks for on scrolling up, 1 to 100 (default 20);
    /// each later page asks for more, up to `server.history.maxTurns`.
    #[serde(default = "default_page_turns")]
    #[schemars(range(min = 1, max = 100))]
    pub page_turns: u32,
    /// Whether the web opens a thread from its history (default false). `false` opens it as it
    /// always did, by replaying the whole log.
    #[serde(default)]
    pub windowed: bool,
}

fn default_initial_turns() -> u32 {
    12
}

fn default_page_turns() -> u32 {
    20
}

impl Default for UiHistory {
    fn default() -> Self {
        UiHistory {
            initial_turns: default_initial_turns(),
            page_turns: default_page_turns(),
            windowed: false,
        }
    }
}

/// The most `artifacts.maxFileBytes` may be, 256 MiB: a file is written whole (it is held in memory
/// once), so the cap is a memory bound as much as a policy.
pub const MAX_FILE_BYTES_LIMIT: u64 = 256 * 1024 * 1024;

/// The default of `artifacts.maxFileBytes`, 10 MiB.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// The most `artifacts.maxPerJobBytes` may be, 4 GiB.
pub const MAX_PER_JOB_BYTES_LIMIT: u64 = 4 * 1024 * 1024 * 1024;

/// The default of `artifacts.maxPerJobBytes`, 100 MiB.
pub const DEFAULT_MAX_PER_JOB_BYTES: u64 = 100 * 1024 * 1024;

/// The default of `artifacts.s3.region`.
pub const DEFAULT_S3_REGION: &str = "us-east-1";

/// Which store keeps the files, one of the implementations of the `ArtifactStore` port. A build
/// that does not have the Cargo feature of the one named refuses it (exit 78), never ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactStoreKind {
    /// A directory (`artifacts.fs.root`), for development and a single node. Needs the Cargo
    /// feature `artifacts-fs`. Every role of the deployment must see the same directory.
    Fs,
    /// An S3 bucket (`artifacts.s3`), AWS or any S3-compatible server. Needs the Cargo feature
    /// `artifacts-s3`.
    S3,
}

impl ArtifactStoreKind {
    /// The name used in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            ArtifactStoreKind::Fs => "fs",
            ArtifactStoreKind::S3 => "s3",
        }
    }

    /// The Cargo feature of the `orchestrator` package that compiles the store in.
    pub const fn feature(self) -> &'static str {
        match self {
            ArtifactStoreKind::Fs => "artifacts-fs",
            ArtifactStoreKind::S3 => "artifacts-s3",
        }
    }
}

/// The artifact store (ADR 0032): the files an agent hands over, kept by the hash of their
/// content. The event log keeps only the reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifacts {
    /// Which store: `fs` (a directory, for development and one node) or `s3` (a bucket). The
    /// section of the store chosen is required, and the other one is an error.
    pub store: ArtifactStoreKind,
    /// The directory store. Required with `store: fs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs: Option<ArtifactsFs>,
    /// The S3 store. Required with `store: s3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<ArtifactsS3>,
    /// The largest file kept, in bytes: 1 to 268435456 (256 MiB), default 10485760 (10 MiB). A
    /// larger file is not kept: the agent's artifact is shown without it, with an error.
    #[serde(default = "default_max_file_bytes")]
    #[schemars(range(min = 1, max = 268_435_456))]
    pub max_file_bytes: u64,
    /// The most bytes of files one job keeps (a job is one run of an agent): 1 to 4294967296
    /// (4 GiB), default 104857600 (100 MiB). A job also keeps at most 50 files. A file that would go
    /// over either is not kept, like one over `maxFileBytes`.
    #[serde(default = "default_max_per_job_bytes")]
    #[schemars(range(min = 1, max = 4_294_967_296u64))]
    pub max_per_job_bytes: u64,
    /// The hosts a `url` part of an agent's artifact may be fetched from (`files.example.com`,
    /// `10.0.0.5:8080`: a host name or address, with or without a port, no scheme, path, wildcard
    /// or credentials). Default none: a `url` part stays a link. A host here is **trusted**: the
    /// orchestrator reads a file from it on an agent's say-so, over `http` or `https`, with no
    /// redirect followed, no credential sent, within `maxFileBytes`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fetch_hosts: Vec<String>,
}

fn default_max_file_bytes() -> u64 {
    DEFAULT_MAX_FILE_BYTES
}

fn default_max_per_job_bytes() -> u64 {
    DEFAULT_MAX_PER_JOB_BYTES
}

/// The directory store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactsFs {
    /// The directory the files are kept in; relative to this file's directory. It is made (mode
    /// 0700) when it is not there. The worker that keeps a file and the control plane that serves
    /// it must see the same directory: one machine, or a shared volume.
    #[schemars(length(min = 1))]
    pub root: String,
}

/// The S3 store. The bucket is addressed in the path of the endpoint when there is one, and as a
/// host name of AWS otherwise. The credentials are static: this build does not read `AWS_*`
/// variables or an instance profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactsS3 {
    /// The bucket, which must exist: 3 to 63 characters, lower case letters, digits, `-` and `.`,
    /// starting and ending with a letter or a digit.
    pub bucket: String,
    /// The region (default `us-east-1`, which S3-compatible servers that have none expect).
    #[serde(default = "default_s3_region")]
    #[schemars(length(min = 1, max = 32))]
    pub region: String,
    /// The server's URL, `http` or `https`, for a server other than AWS (`https://minio.example.com`).
    /// Absent: AWS S3 in `region`. An `http` endpoint sends the files in the clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// A prefix every key is put under (`orchestrator/prod`: `a-z`, `A-Z`, `0-9`, `.`, `_`, `-`
    /// and `/`, no `..`, at most 128 characters), so one bucket serves several deployments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 128))]
    pub prefix: Option<String>,
    /// The access key id, a secret.
    pub access_key_id: SecretRef,
    /// The secret access key, a secret.
    pub secret_access_key: SecretRef,
    /// Seconds one request may take, 1 to 3600 (default 60).
    #[serde(default = "default_s3_timeout_secs")]
    #[schemars(range(min = 1, max = 3600))]
    pub timeout_secs: u64,
}

fn default_s3_region() -> String {
    DEFAULT_S3_REGION.to_owned()
}

fn default_s3_timeout_secs() -> u64 {
    60
}

/// The thread tools (`thread-tools/v1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadTools {
    /// The base URL under which agents reach this orchestrator, `http` or `https`. Both or
    /// neither of `url` and `secret`. Replaces `THREAD_TOOLS_URL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The HMAC key of the tokens, a secret of at least 32 bytes. Replaces
    /// `THREAD_TOOLS_SECRET`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretRef>,
    /// The previous key, for verifying only (a rotation): a secret of at least 32 bytes, not the
    /// current one. Replaces `THREAD_TOOLS_SECRET_PREVIOUS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_secret: Option<SecretRef>,
    /// Seconds a token lives, 60 to 86400 (default 7200). Replaces
    /// `THREAD_TOOLS_TOKEN_TTL_SECS`.
    #[serde(default = "default_token_ttl_secs")]
    #[schemars(range(min = 60, max = 86400))]
    pub token_ttl_secs: u64,
    /// The `Host` values the endpoint accepts, each `host` or `host:port` (default: the host of
    /// `url`). Replaces `THREAD_TOOLS_ALLOWED_HOSTS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub allowed_hosts: Option<Vec<String>>,
}

fn default_token_ttl_secs() -> u64 {
    7200
}

impl Default for ThreadTools {
    fn default() -> Self {
        ThreadTools {
            url: None,
            secret: None,
            previous_secret: None,
            token_ttl_secs: default_token_ttl_secs(),
            allowed_hosts: None,
        }
    }
}

/// What a deployment allows a thread to be shared as (ADR 0040): the **cap** on every thread's
/// visibility. What is served is the narrower of the thread's own visibility and this, read when a
/// link is opened: lowering it narrows every link at once with no data change, and raising it
/// again brings them back. It is read once at startup, so a change is a restart, which also ends
/// every open stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SharingMode {
    /// Nothing is shared and nothing can be: every link answers 404, and an owner can still take
    /// a link down.
    #[default]
    Disabled,
    /// Signed-in people who have the link may read a thread its owner shared.
    Internal,
    /// Anybody who has the link may read a thread its owner shared, signed in or not. Needs the
    /// rate limit of the public routes, which is always on.
    Public,
}

impl SharingMode {
    /// The name used in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            SharingMode::Disabled => "disabled",
            SharingMode::Internal => "internal",
            SharingMode::Public => "public",
        }
    }
}

/// Sharing a thread by a revocable link (ADR 0040).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sharing {
    /// The cap: `disabled` (the default), `internal` or `public`.
    #[serde(default)]
    pub mode: SharingMode,
    /// The secret a link's MAC is made under: a secret of at least 32 bytes, never the same as
    /// `threadTools.secret`. Required unless `mode` is `disabled`. Losing it ends every link until
    /// the owners copy them again (the server recomputes a link from the thread's row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretRef>,
    /// The previous secret, for verifying only (a rotation): links made under either still open.
    /// A secret of at least 32 bytes, not the current one. Needs `secret`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_secret: Option<SecretRef>,
    /// What a public reader may see beyond the default. Only with `mode: public`: a key that does
    /// nothing is an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<SharingPublic>,
    /// The limits of the public routes, per process. Only with `mode: public`; the defaults are
    /// the ADR's starting numbers, not yet measured under load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<SharingRateLimit>,
}

/// What a public reader sees beyond the default (`sharing.public`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharingPublic {
    /// A public reader sees the input and output of steps, not only their labels (default
    /// `false`). They are already redacted and capped (ADR 0030), and are where tools echo the
    /// most.
    #[serde(default)]
    pub step_io: bool,
    /// A public reader can open the thread's files (default `false`).
    #[serde(default)]
    pub files: bool,
}

/// The limits of the public routes (ADR 0040, section 10): a token bucket per link and one for all
/// of them, and a ceiling on open streams. Every failure of these routes counts against the shared
/// bucket, so guessing tokens is throttled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharingRateLimit {
    /// Requests a second for one link, 1 to 1000 (default 10).
    #[serde(default = "default_per_link")]
    #[schemars(range(min = 1, max = 1000))]
    pub per_link_per_second: u64,
    /// Requests a second for all links together, 1 to 10000 (default 100).
    #[serde(default = "default_total")]
    #[schemars(range(min = 1, max = 10000))]
    pub total_per_second: u64,
    /// Open streams for one link, 1 to 100 (default 5).
    #[serde(default = "default_streams_per_link")]
    #[schemars(range(min = 1, max = 100))]
    pub streams_per_link: u64,
    /// Open streams for all links together, 1 to 1000 (default 50).
    #[serde(default = "default_streams_total")]
    #[schemars(range(min = 1, max = 1000))]
    pub streams_total: u64,
}

fn default_per_link() -> u64 {
    10
}

fn default_total() -> u64 {
    100
}

fn default_streams_per_link() -> u64 {
    5
}

fn default_streams_total() -> u64 {
    50
}

impl Default for SharingRateLimit {
    fn default() -> Self {
        SharingRateLimit {
            per_link_per_second: default_per_link(),
            total_per_second: default_total(),
            streams_per_link: default_streams_per_link(),
            streams_total: default_streams_total(),
        }
    }
}

/// The MCP server surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mcp {
    /// The tokens file: a YAML list of `{user, tokenEnv}`, relative to this file's directory.
    /// Required when `mcp` is mounted by a role that serves routes. Replaces `MCP_TOKENS_FILE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub tokens_file: Option<String>,
    /// The `Host` values the server accepts, each `host` or `host:port`. Required when `mcp` is
    /// mounted by a role that serves routes. Replaces `MCP_ALLOWED_HOSTS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub allowed_hosts: Option<Vec<String>>,
    /// Browser origins let through, each `http(s)://host[:port]` (default none). Replaces
    /// `MCP_ALLOWED_ORIGINS`.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// The largest `timeout_secs` of `wait_for_job`, 1 to 86400 (default 3600). Replaces
    /// `MCP_WAIT_MAX_SECS`.
    #[serde(default = "default_wait_max_secs")]
    #[schemars(range(min = 1, max = 86400))]
    pub wait_max_secs: u64,
    /// The most `wait_for_job` calls this process holds open, at least 1 (default 256). Replaces
    /// `MCP_WAIT_MAX_CONCURRENT`.
    #[serde(default = "default_wait_max_concurrent")]
    #[schemars(range(min = 1))]
    pub wait_max_concurrent: u64,
    /// The most one user may hold open, at least 1 (default 16). Replaces
    /// `MCP_WAIT_MAX_PER_USER`.
    #[serde(default = "default_wait_max_per_user")]
    #[schemars(range(min = 1))]
    pub wait_max_per_user: u64,
}

fn default_wait_max_secs() -> u64 {
    3600
}

fn default_wait_max_concurrent() -> u64 {
    256
}

fn default_wait_max_per_user() -> u64 {
    16
}

impl Default for Mcp {
    fn default() -> Self {
        Mcp {
            tokens_file: None,
            allowed_hosts: None,
            allowed_origins: Vec::new(),
            wait_max_secs: default_wait_max_secs(),
            wait_max_concurrent: default_wait_max_concurrent(),
            wait_max_per_user: default_wait_max_per_user(),
        }
    }
}

/// The webhook surfaces.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Webhooks {
    /// `POST /webhooks/ci`. Required when the surface `webhook-generic` is mounted by a role
    /// that serves routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic: Option<WebhookGeneric>,
    /// `POST /webhooks/github`. Required when the surface `webhook-github` is mounted by a role
    /// that serves routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<WebhookGithub>,
}

/// The generic signed CI report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookGeneric {
    /// One or two shared secrets, each at least 32 bytes: a report is accepted when its
    /// signature matches either, so a secret can be rotated. Replaces `WEBHOOK_GENERIC_SECRETS`
    /// (one variable holding one or two comma-separated secrets).
    #[schemars(length(min = 1, max = 2))]
    pub secrets: Vec<SecretRef>,
    /// Seconds the `X-Vymalo-Timestamp` may differ from the clock, either way, at least 1
    /// (default 300). Replaces `WEBHOOK_GENERIC_MAX_SKEW_SECS`.
    #[serde(default = "default_max_skew_secs")]
    #[schemars(range(min = 1))]
    pub max_skew_secs: u64,
}

fn default_max_skew_secs() -> u64 {
    300
}

/// GitHub's own `check_run` and `workflow_run` deliveries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookGithub {
    /// One or two secrets of the GitHub webhook, each at least 32 bytes. Replaces
    /// `WEBHOOK_GITHUB_SECRETS` (one variable holding one or two comma-separated secrets).
    #[schemars(length(min = 1, max = 2))]
    pub secrets: Vec<SecretRef>,
    /// Seconds old the signed completion time of an event may be, at least 1 (default 86400).
    /// Replaces `WEBHOOK_GITHUB_MAX_AGE_SECS`.
    #[serde(default = "default_github_max_age_secs")]
    #[schemars(range(min = 1))]
    pub max_age_secs: u64,
}

fn default_github_max_age_secs() -> u64 {
    86_400
}

/// Authentication and what a person may do (ADR 0033).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Auth {
    /// How a request says who it is from (default `proxy_header`, which changes nothing):
    /// `proxy_header` trusts the identity header oauth2-proxy sets (and `devUser`), `jwt` is an
    /// OAuth2 resource server that validates the bearer token against the issuer's keys (it
    /// needs `jwt`), `jwt_or_proxy_header` takes the token when there is one and the header when
    /// there is not (the migration, for one release). A mode whose Cargo feature (`auth-jwt`,
    /// `auth-header`) is not in the build is refused.
    #[serde(default)]
    pub mode: AuthMode,
    /// The token issuer and what is read from its tokens. Required by the modes that read tokens,
    /// refused by `proxy_header`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwt: Option<Jwt>,
    /// DPoP-bound tokens (RFC 9449, ADR 0054): the orchestrator then also takes `Authorization: DPoP
    /// <token>` with a `DPoP` proof, and refuses a token that is bound to a key when it is sent as
    /// `Bearer`. Absent (the default), the `DPoP` scheme is refused and bearer tokens are exactly what
    /// they were. Only with a mode that reads tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpop: Option<AuthDpop>,
    /// How the web signs a person in by itself (ADR 0054): its public client of the issuer, served at
    /// `GET /api/public/auth` so that the web needs no configuration of its own. Absent (the default),
    /// that route is a 404 and the web keeps the edge's cookie. Needs a mode that reads tokens and
    /// `dpop`, since the web's tokens are bound to its key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<AuthBrowser>,
    /// The e-mail served for requests without `X-Auth-Request-Email`. Development only: the
    /// orchestrator logs a warning, and only `auth.mode: proxy_header` takes it. Replaces
    /// `AUTH_DEV_USER`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub dev_user: Option<String>,
    /// The role of a person none of whose roles is one of `roles`: a valid token whose roles claim
    /// holds none that is defined here, or no roles claim at all, and every request of the proxy
    /// header, which carries none. It must be one of `roles`. Written `null`, nobody gets a role by
    /// default: such a person is refused (403) by everything but `GET /api/me`. Absent, it is
    /// `user` when `roles` is absent too (so that a deployment that configures nothing is as it
    /// was), and `null` when `roles` is given (a deployment that defines its roles names the
    /// default, or has none).
    #[serde(
        default,
        deserialize_with = "default_role_value",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "default_role_schema")]
    pub default_role: Option<Option<String>>,
    /// What each role grants, by the name the identity provider gives the role (a group, a realm
    /// role: `auth.jwt.rolesClaim`), compared exactly. A role that is not defined here grants
    /// nothing. Absent: `user` (everything but `admin`, over one's own threads) and `admin` (also
    /// `admin`: operational, and no more access to threads than a user has, ADR 0039). Given, it
    /// replaces both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<BTreeMap<String, AuthRole>>,
}

/// How DPoP proofs are checked (`auth.dpop`, RFC 9449, ADR 0054).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthDpop {
    /// The origins the clients call this API at, for example `https://chat.example.com`: a proof's
    /// `htu` must be one of them followed by the request's path. The orchestrator sits behind a
    /// proxy and cannot rebuild the public URL itself. Each is an absolute `http(s)` origin with no
    /// path, query or fragment, and at least one is needed. In production `http://` is refused, but
    /// for `localhost`, `127.0.0.1` and `[::1]`.
    #[schemars(length(min = 1))]
    pub public_origins: Vec<String>,
    /// Seconds old a proof's `iat` may be, 1 to 600 (default 60). A proof's `jti` is remembered for
    /// this long and `futureSkewSeconds` more, in the memory of the process.
    #[serde(default = "default_dpop_max_age_seconds")]
    #[schemars(range(min = 1, max = 600))]
    pub max_age_seconds: u64,
    /// Seconds a proof's `iat` may be ahead of this process's clock, 0 to 60 (default 5).
    #[serde(default = "default_dpop_future_skew_seconds")]
    #[schemars(range(min = 0, max = 60))]
    pub future_skew_seconds: u64,
}

/// The default of `auth.dpop.maxAgeSeconds`.
pub const DEFAULT_DPOP_MAX_AGE_SECONDS: u64 = 60;

/// The default of `auth.dpop.futureSkewSeconds`.
pub const DEFAULT_DPOP_FUTURE_SKEW_SECONDS: u64 = 5;

fn default_dpop_max_age_seconds() -> u64 {
    DEFAULT_DPOP_MAX_AGE_SECONDS
}

fn default_dpop_future_skew_seconds() -> u64 {
    DEFAULT_DPOP_FUTURE_SKEW_SECONDS
}

/// The web's sign-in (`auth.browser`, ADR 0054): what `GET /api/public/auth` says besides the issuer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthBrowser {
    /// The web's OAuth client id at the issuer (`auth.jwt.issuer`): a public client, so there is no
    /// secret to give. Non-empty, no space.
    #[schemars(length(min = 1))]
    pub client_id: String,
    /// The scope the web asks for: scope tokens separated by single spaces (default
    /// `openid email profile offline_access`).
    #[serde(default = "default_browser_scope")]
    #[schemars(length(min = 1))]
    pub scope: String,
}

/// The default of `auth.browser.scope`.
pub const DEFAULT_BROWSER_SCOPE: &str = "openid email profile offline_access";

fn default_browser_scope() -> String {
    DEFAULT_BROWSER_SCOPE.to_owned()
}

/// `Some(None)` for `defaultRole: null`, `Some(Some(name))` for a name; `None` (absent) is the
/// serde default. `Option<Option<T>>` alone reads `null` as absent.
fn default_role_value<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// The schema of `auth.defaultRole`: a non-empty text, or `null`, which is a value here (nobody
/// gets a role by default) and so is kept when the schema is tidied (`x-null-is-a-value`).
fn default_role_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "description": "The role of a person none of whose roles is one of `roles`; `null` for no default role.",
        "type": ["string", "null"],
        "minLength": 1,
        "x-null-is-a-value": true
    })
}

/// What a role grants (ADR 0033).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthRole {
    /// The permissions the role holds. Empty: the role is known and grants nothing.
    pub permissions: Vec<AuthPermission>,
    /// How far `thread.read`, `artifact.read` and `thread.write` reach: `own`, the person's own
    /// threads, which is the default and the only scope there is. `any` is refused: no role reads
    /// or acts on another person's thread (ADR 0039); share the thread instead. The key is kept
    /// for `version: 1` files. Only with a role that holds one of those three.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<AuthScopes>,
    /// The agents `agent.read` and `agent.invoke` are about: agent ids, or `"*"` for every agent
    /// (default `["*"]`). An id that no agent has matches nothing. Only with a role that holds
    /// one of those two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<String>>,
}

/// A permission of a role. The names are the platform's
/// (`another-agentic-platform` `docs/architecture/08-security.md`, section 52).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub enum AuthPermission {
    /// See an agent in the list and read its card.
    #[serde(rename = "agent.read")]
    AgentRead,
    /// Start a thread on an agent, or send it a message.
    #[serde(rename = "agent.invoke")]
    AgentInvoke,
    /// Read threads: the thread, its log and stream, its export, its branches.
    #[serde(rename = "thread.read")]
    ThreadRead,
    /// Act on threads: start one, send, answer, cancel, rename, describe, fork.
    #[serde(rename = "thread.write")]
    ThreadWrite,
    /// Download the files of the threads the person may read.
    #[serde(rename = "artifact.read")]
    ArtifactRead,
    /// Set, widen, narrow or make a new link for the visibility of one's own thread (ADR 0040).
    /// It takes no scope. Taking a link down is not gated by it: the owner can always revoke.
    #[serde(rename = "thread.share")]
    ThreadShare,
    /// Delete one's own thread: it is erased with its edits and its files (ADR 0043). It takes no
    /// scope and does not need `thread.write`. A role that lists its permissions does not get it by
    /// itself.
    #[serde(rename = "thread.delete")]
    ThreadDelete,
    /// Operational and content-free (ADR 0039): it reaches no person's thread, file or listing.
    /// It names what an endpoint that shows an operator no content may be used by.
    #[serde(rename = "admin")]
    Admin,
}

impl AuthPermission {
    /// The name used in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthPermission::AgentRead => "agent.read",
            AuthPermission::AgentInvoke => "agent.invoke",
            AuthPermission::ThreadRead => "thread.read",
            AuthPermission::ThreadWrite => "thread.write",
            AuthPermission::ThreadShare => "thread.share",
            AuthPermission::ThreadDelete => "thread.delete",
            AuthPermission::ArtifactRead => "artifact.read",
            AuthPermission::Admin => "admin",
        }
    }
}

/// How far a permission over threads reaches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthScope {
    /// The threads the person owns: the only scope there is.
    #[default]
    Own,
    /// Everyone's threads. **Refused by the file's rules** (ADR 0039): it is read only so that the
    /// error can name the key and the decision, instead of a type error that says nothing.
    Any,
}

/// The `scope` of a role: one scope for everything, or one for reading and one for writing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AuthScopes {
    /// `own`, for reading and for writing alike (`any` is refused, ADR 0039).
    Both(AuthScope),
    /// `{ read: own, write: own }`, each member defaulting to `own` (`any` is refused, ADR 0039).
    Split(SplitScope),
}

/// A scope for reading (`thread.read`, `artifact.read`) and one for writing (`thread.write`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitScope {
    /// How far reading reaches (default `own`).
    #[serde(default)]
    pub read: AuthScope,
    /// How far acting reaches (default `own`).
    #[serde(default)]
    pub write: AuthScope,
}

impl AuthScopes {
    /// The scope of reading and the scope of writing.
    pub fn read_write(&self) -> (AuthScope, AuthScope) {
        match self {
            AuthScopes::Both(scope) => (*scope, *scope),
            AuthScopes::Split(split) => (split.read, split.write),
        }
    }
}

/// How a request says who it is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// The identity header oauth2-proxy sets after the login. Trustworthy only behind a proxy
    /// that strips client-supplied copies; refused when `server.environment` is `production`.
    #[default]
    ProxyHeader,
    /// An OAuth2 resource server: only a bearer token that validates is an identity.
    Jwt,
    /// A request with a bearer token is the token's (and a bad token is never reconsidered as the
    /// header); a request without one is the identity header's. For the one release of migration.
    JwtOrProxyHeader,
}

impl AuthMode {
    /// The name used in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthMode::ProxyHeader => "proxy_header",
            AuthMode::Jwt => "jwt",
            AuthMode::JwtOrProxyHeader => "jwt_or_proxy_header",
        }
    }

    /// Whether this mode reads bearer tokens (it needs `auth.jwt` and the feature `auth-jwt`).
    pub const fn reads_tokens(self) -> bool {
        matches!(self, AuthMode::Jwt | AuthMode::JwtOrProxyHeader)
    }

    /// Whether this mode reads the identity header (it needs the feature `auth-header`).
    pub const fn reads_header(self) -> bool {
        matches!(self, AuthMode::ProxyHeader | AuthMode::JwtOrProxyHeader)
    }
}

/// The token issuer, and what is read from its tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Jwt {
    /// The issuer: a token's `iss` must equal it exactly, and unless `jwksUrl` is set its keys
    /// are found from `<issuer>/.well-known/openid-configuration`. An `http(s)` URL with no
    /// credentials, query or fragment, for example `https://idp.example/realms/main`.
    pub issuer: String,
    /// The audiences this API accepts: one of them must be among the token's `aud`. With
    /// oauth2-proxy passing the ID token this is oauth2-proxy's client id.
    #[schemars(length(min = 1))]
    pub audiences: Vec<String>,
    /// Where the issuer's keys (a JWKS) are, when not in its discovery document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_url: Option<String>,
    /// The claim whose value is the user, and so the owner of what the person creates (default
    /// `email`, which keeps the threads that exist). Trimmed and lower-cased.
    #[serde(default = "default_user_claim")]
    #[schemars(length(min = 1))]
    pub user_claim: String,
    /// The path of the claim that holds the person's roles, with dots for the objects on the way:
    /// `realm_access.roles` (Keycloak), `groups`. None: no roles. A claim whose own name has dots
    /// (`https://example.com/roles`) is found by that name first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub roles_claim: Option<String>,
}

fn default_user_claim() -> String {
    "email".to_owned()
}

/// The default of `toolServers[].timeoutSecs`.
pub const DEFAULT_TOOL_SERVER_TIMEOUT_SECS: u64 = 120;

/// An MCP server a person may attach to a conversation (ADR 0024, `thread-tools/v1`). The orchestrator calls it for the agent and holds its credentials: they are secret references
/// and appear in no event, no API answer, no log line and no agent message. A person cannot enter
/// a URL: this list is the deployment's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolServer {
    /// The server's id: `a-z`, `0-9` and `-`, starting with a letter or a digit, 1 to 31
    /// characters, unique in the list. No `_`, so the first `__` of a relayed tool's name
    /// (`<id>__<tool>`) is the split.
    #[schemars(length(min = 1))]
    pub id: String,
    /// The name the picker and the steps show, 1 to 80 characters.
    #[schemars(length(min = 1))]
    pub name: String,
    /// What the server is for, at most 500 characters. The web shows it, and the agent is told it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The server's MCP endpoint (streamable HTTP), `http` or `https`, with a host and no user
    /// name, password, query or fragment: a credential goes in `bearer` or `headers`.
    #[schemars(length(min = 1))]
    pub url: String,
    /// The icon the web draws for the server and for its steps: a `data:` URI,
    /// `data:image/svg+xml;base64,...` (or `image/png`, `image/webp`), at most 8 KiB. An icon at a
    /// URL is never fetched (open question 38), and the icons the server itself offers are
    /// dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// A bearer token the server wants, sent as `Authorization: Bearer <value>` on the
    /// orchestrator's own requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer: Option<SecretRef>,
    /// Further headers the server wants: the header's name to a secret reference that holds its
    /// value. `Authorization` (use `bearer`), `Accept`, `Content-Type`, `Host`, `Mcp-Session-Id`,
    /// `Mcp-Protocol-Version` and `Last-Event-ID` are refused.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, SecretRef>,
    /// An allow-list of the server's own tool names to relay (default: every tool whose name the
    /// relay can expose: `a-z`, `A-Z`, `0-9`, `_` and `-`, not starting with `_`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub tools: Option<Vec<String>>,
    /// The ids of the agents the server may be attached for (default: every agent). A thread
    /// whose agent is not listed cannot attach it (422).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub agents: Option<Vec<String>>,
    /// Seconds one call may take, 1 to 600 (default 120).
    #[serde(default = "default_tool_server_timeout_secs")]
    #[schemars(range(min = 1, max = 600))]
    pub timeout_secs: u64,
}

fn default_tool_server_timeout_secs() -> u64 {
    DEFAULT_TOOL_SERVER_TIMEOUT_SECS
}
