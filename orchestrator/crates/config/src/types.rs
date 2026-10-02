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
    /// The OpenAI-compatible endpoints the orchestrator asks itself (ADR 0005, ADR 0035).
    #[serde(default)]
    pub models: Models,
    /// The utility tasks that use a model: a task that is absent is off (ADR 0035).
    #[serde(default)]
    pub tasks: Tasks,
    /// The thread tools: the endpoint agents call back, and the key of its tokens.
    #[serde(default)]
    pub thread_tools: ThreadTools,
    /// The MCP server surface (ADR 0019).
    #[serde(default)]
    pub mcp: Mcp,
    /// The CI webhook surfaces (ADR 0017).
    #[serde(default)]
    pub webhooks: Webhooks,
    /// Authentication.
    #[serde(default)]
    pub auth: Auth,
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

/// The model endpoints.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Models {
    /// Endpoints by name (a slug: `a-z`, `0-9` and `-`, up to 32 characters). This build takes
    /// one endpoint; the legacy `ORCH_MODEL_*` variables are the endpoint named `default`.
    #[serde(default)]
    pub endpoints: BTreeMap<String, Endpoint>,
}

/// An OpenAI-compatible endpoint (ADR 0005).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    /// The base URL, up to and not including `/chat/completions`, `http` or `https`. Replaces
    /// `ORCH_MODEL_BASE_URL`.
    pub base_url: String,
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

/// The utility tasks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tasks {
    /// The thread title. Absent: titles are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<TitleTask>,
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

/// Authentication.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Auth {
    /// The e-mail served for requests without `X-Auth-Request-Email`. Development only: the
    /// orchestrator logs a warning. Replaces `AUTH_DEV_USER`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub dev_user: Option<String>,
}
