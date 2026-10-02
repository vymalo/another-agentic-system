//! Configuration: command-line flags with environment fallback, and the `AGENTS_FILE`.
//!
//! [`Args`] is the clap layer: every setting is a flag whose value falls back to the
//! environment variable the service has always read (`--listen-addr` / `LISTEN_ADDR`, and so
//! on), so deployments do not change. Clap only *collects* the raw strings. Everything is then
//! validated by [`Config::load`], which is pure: the file system and the variables named by an
//! agent's `tokenEnv` are read through closures, and the tests build [`Args`] by hand, so they
//! never touch the process environment. Keeping the validation out of clap keeps one error
//! type: every problem is a [`ConfigError`] that names the variable, file or agent at fault,
//! carries no secret value, and exits with the configuration code (78) rather than clap's
//! usage code (2). Startup fails fast and fails closed.

mod file;

pub use file::{FlagSecrets, Loaded, secret_flags};
pub use orch_config::AuthMode;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
#[cfg(feature = "surface-thread-tools")]
use std::sync::Arc;
use std::time::Duration;

use adam_host::Role;
use clap::Parser;
use orch_app::{
    AgentDirectory, AgentEntry, AgentScope, AppConfig, DEFAULT_MAX_ATTEMPTS_CAP, GateLayer,
    GateRules, InboxConfig, Layer, MAX_ATTEMPTS_CAP_CEILING, Permission, Policy, PublicConfig,
    RoleGrant, Scope, TaskSettings, ToolServerInfo, built_in_roles, known_sources,
};
use orch_core::{
    AgentId, CheckSource, DEFAULT_CI_TIMEOUT_SECS, DEFAULT_MAX_ATTEMPTS,
    DEFAULT_VERIFIER_TIMEOUT_SECS, GatePolicy, TaskKind, UserId,
};
use orch_ports::{AgentEndpoint, ToolServerEndpoint};
#[cfg(feature = "surface-webhook")]
use orch_surface_webhook::{GenericConfig, GithubConfig, Secrets};
#[cfg(feature = "surface-thread-tools")]
use orch_thread_token::{
    DEFAULT_TTL_SECS, MAX_TTL_SECS, MIN_TTL_SECS, ThreadToolsIssuer, ThreadToolsKeys,
};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use url::Url;

use crate::artifacts::ArtifactSettings;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_DATABASE_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_DISPATCHER_CONCURRENCY: usize = 32;
const DEFAULT_OUTBOX_LEASE_SECS: u64 = 30;
const DEFAULT_VERIFIER_WATCH_SECS: u64 = 5;
const DEFAULT_MODEL_TIMEOUT_SECS: u64 = 20;
/// `AGENT_REGISTRY_TIMEOUT_SECS`, when unset, and the most it may be.
const DEFAULT_REGISTRY_TIMEOUT_SECS: u64 = 3;
const MAX_REGISTRY_TIMEOUT_SECS: u64 = 60;
/// `AGENT_REGISTRY_MAX_AGE_SECS`, when unset, and the most it may be.
const DEFAULT_REGISTRY_MAX_AGE_SECS: u64 = 60;
const MAX_REGISTRY_MAX_AGE_SECS: u64 = 3600;
#[cfg(feature = "agent-local")]
const DEFAULT_AGENT_LOCAL_CONCURRENCY: usize = 4;
const DEFAULT_SHUTDOWN_GRACE_SECS: u64 = 15;
const DEFAULT_MCP_WAIT_MAX_SECS: u64 = 3600;
/// The largest `MCP_WAIT_MAX_SECS`: a day. A wait is a request that stays open.
const MAX_MCP_WAIT_MAX_SECS: u64 = 86_400;
const DEFAULT_MCP_WAIT_MAX_CONCURRENT: usize = 256;
const DEFAULT_MCP_WAIT_MAX_PER_USER: usize = 16;
/// The shortest bearer token: 32 bytes, what `openssl rand -base64 32` gives (43 characters).
/// Static tokens never expire and nothing slows a guess down.
const MIN_MCP_TOKEN_BYTES: usize = 32;
/// `WEBHOOK_GENERIC_MAX_SKEW_SECS`, when unset (ADR 0017).
#[cfg(feature = "surface-webhook")]
const DEFAULT_WEBHOOK_MAX_SKEW_SECS: u64 = orch_surface_webhook::generic::DEFAULT_MAX_SKEW_SECS;

/// A configuration problem. The message is what the operator sees.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required environment variable is unset or empty.
    #[error("{0} is required")]
    Missing(&'static str),
    /// A surface is mounted and a variable it cannot run without is unset or empty. Fail closed:
    /// a webhook route without its secret would be a route anyone can call.
    #[cfg(any(feature = "surface-webhook", feature = "surface-thread-tools"))]
    #[error("{var} is required when ORCH_SURFACES mounts {surface:?}")]
    MissingForSurface {
        /// The variable.
        var: &'static str,
        /// The surface that needs it.
        surface: &'static str,
    },
    /// An environment variable has an unusable value.
    #[error("{var} is invalid: {reason}")]
    Invalid {
        /// The variable.
        var: &'static str,
        /// What is wrong with it.
        reason: String,
    },
    /// `ORCH_SURFACES` names a surface this binary does not know.
    #[error("ORCH_SURFACES is invalid: unknown surface {name:?} (known: {known})")]
    UnknownSurface {
        /// The name as written.
        name: String,
        /// The known names, comma separated.
        known: String,
    },
    /// `ORCH_SURFACES` names a surface that used to exist and was removed. Fail closed, with
    /// the way forward: a deployment that still lists it is not quietly started without the
    /// routes it expects.
    #[error(
        "ORCH_SURFACES is invalid: surface {name:?} was removed on {removed}: {what}. \
         Use AG-UI instead: set ORCH_SURFACES=agui (the default) and speak {replacement}"
    )]
    RemovedSurface {
        /// The surface's name as written.
        name: &'static str,
        /// The date it was removed (ISO 8601).
        removed: &'static str,
        /// What it was.
        what: &'static str,
        /// What replaces it.
        replacement: &'static str,
    },
    /// `ORCH_SURFACES` names a surface whose Cargo feature was not compiled in.
    #[error(
        "ORCH_SURFACES is invalid: surface {surface:?} is not in this build \
         (enable the Cargo feature {feature:?})"
    )]
    SurfaceNotCompiled {
        /// The surface's name.
        surface: &'static str,
        /// The Cargo feature of the `orchestrator` package that provides it.
        feature: &'static str,
    },
    /// `AGENT_REGISTRY_URL` is set, and this build has no platform registry. Fail closed, like a
    /// surface that is not compiled in: the setting is never quietly ignored.
    #[cfg(not(feature = "registry-platform"))]
    #[error(
        "AGENT_REGISTRY_URL is set, and this build has no platform registry; it needs the Cargo \
         feature \"registry-platform\" (ADR 0022)"
    )]
    RegistryNotCompiled,
    /// An `AGENTS_FILE` entry asks for `transport: local`, and this build has no in-process
    /// agents. Fail closed, like a surface that is not compiled in: the entry is never quietly
    /// dropped or served by the A2A client.
    #[error(
        "agent {agent:?}: transport \"local\" is not in this build; it needs the Cargo feature \
         {feature:?}: build the orchestrator with `--features {feature}` (ADR 0015)"
    )]
    LocalAgentsNotCompiled {
        /// The agent entry's id.
        agent: String,
        /// The Cargo feature of the `orchestrator` package that would compile local agents in.
        feature: &'static str,
    },
    /// The artifact store `artifacts.store` names cannot be used: its directory cannot be made or
    /// written, or its bucket is not configured correctly. The text names the key and never holds a
    /// credential.
    #[error("{0}")]
    Artifacts(String),
    /// `auth.mode` names an authenticator whose Cargo feature was not compiled in. Fail closed,
    /// like a surface that is not compiled in: the setting is never quietly ignored, and a
    /// process that cannot authenticate is not started to refuse everybody.
    #[error(
        "auth.mode {mode} is not in this build; it needs the Cargo feature {feature:?} (ADR 0033)"
    )]
    AuthNotCompiled {
        /// The mode, as written in the file.
        mode: &'static str,
        /// The Cargo feature of the `orchestrator` package that provides what the mode needs.
        feature: &'static str,
    },
    /// `MCP_TOKENS_FILE` could not be read.
    #[error("cannot read MCP_TOKENS_FILE {}", path.display())]
    McpTokensFileRead {
        /// The path from `MCP_TOKENS_FILE`.
        path: PathBuf,
        /// The I/O error.
        source: io::Error,
    },
    /// The environment variable named by a `tokenEnv` of `MCP_TOKENS_FILE` is unset or empty. A
    /// token that is quietly missing would leave a user locked out, or worse, an MCP surface
    /// that starts with fewer tokens than the operator meant.
    #[error(
        "MCP_TOKENS_FILE: the token of {user:?} is in {var}, but that environment variable is \
         unset or empty"
    )]
    McpTokenEnvMissing {
        /// The user the token is for.
        user: String,
        /// The environment variable that should hold the token.
        var: String,
    },
    /// `AGENTS_FILE` could not be read.
    #[error("cannot read AGENTS_FILE {}", path.display())]
    AgentsFileRead {
        /// The path from `AGENTS_FILE`.
        path: PathBuf,
        /// The I/O error.
        source: io::Error,
    },
    /// `AGENTS_FILE` is not the expected YAML.
    #[error("AGENTS_FILE {} is not a valid agent list: {message}", path.display())]
    AgentsFileParse {
        /// The path from `AGENTS_FILE`.
        path: PathBuf,
        /// The parser's message.
        message: String,
    },
    /// `AGENTS_FILE` lists no agent, so no thread could ever be created.
    #[error("AGENTS_FILE {} lists no agents", path.display())]
    NoAgents {
        /// The path from `AGENTS_FILE`.
        path: PathBuf,
    },
    /// Two entries share an id.
    #[error("agent id {0:?} is listed more than once")]
    DuplicateAgent(String),
    /// An agent entry is unusable.
    #[error("agent {id:?}: {reason}")]
    InvalidAgent {
        /// The entry's id.
        id: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The verification gate is configured in a way the rules refuse (ADR 0018): a source or
    /// setting this build cannot honour yet, a layer that removes what the one above requires,
    /// attempts outside the cap, a verifier that is not another configured agent.
    #[error("{context}: {reason}")]
    Gate {
        /// Where: `AGENTS_FILE`.
        context: &'static str,
        /// What is wrong, naming the agent.
        reason: String,
    },
    /// The configuration file (`ORCH_CONFIG_FILE`, ADR 0034) has errors: every one it could find,
    /// one line each, each naming a key path and never a value.
    #[error("the configuration has {} error(s): {}", .0.len(), .0.join("; "))]
    Document(Vec<String>),
    /// The environment variable named by `tokenEnv` is unset or empty. Sending requests
    /// without the credential the operator asked for would be the unsafe reading.
    #[error(
        "agent {agent:?}: tokenEnv names {var}, but that environment variable is unset or empty"
    )]
    TokenEnvMissing {
        /// The agent's id.
        agent: String,
        /// The environment variable that should hold the token.
        var: String,
    },
}

/// The MCP server's configuration (ADR 0019), read when the surface `mcp` is mounted.
#[derive(Clone)]
pub struct McpSettings {
    /// `MCP_TOKENS_FILE`, resolved: each `tokenEnv` already read, with the roles of the entry
    /// (its `role`, or none). A user may have several tokens.
    pub tokens: Vec<(UserId, BTreeSet<orch_ports::Role>, SecretString)>,
    /// `MCP_ALLOWED_HOSTS`: the `Host` values the server accepts.
    pub allowed_hosts: Vec<String>,
    /// `ORCH_PUBLIC_URL`: the chat's public origin, for the `web_url` of `start_job`.
    pub public_url: Option<String>,
    /// `MCP_WAIT_MAX_SECS`: the largest `timeout_secs` of `wait_for_job`.
    pub wait_max: Duration,
    /// `MCP_WAIT_MAX_CONCURRENT`: the most `wait_for_job` calls this process holds open.
    pub wait_max_concurrent: usize,
    /// `MCP_WAIT_MAX_PER_USER`: the most one user may hold open.
    pub wait_max_per_user: usize,
    /// `MCP_ALLOWED_ORIGINS`: browser origins let through; none by default.
    pub allowed_origins: Vec<String>,
}

impl fmt::Debug for McpSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpSettings")
            .field(
                "tokens",
                &self
                    .tokens
                    .iter()
                    .map(|(user, ..)| user)
                    .collect::<Vec<_>>(),
            )
            .field("allowed_hosts", &self.allowed_hosts)
            .field("public_url", &self.public_url)
            .field("wait_max", &self.wait_max)
            .field("wait_max_concurrent", &self.wait_max_concurrent)
            .field("wait_max_per_user", &self.wait_max_per_user)
            .field("allowed_origins", &self.allowed_origins)
            .finish()
    }
}

/// The thread-tools endpoint's configuration (`thread-tools/v1`), read when `THREAD_TOOLS_SECRET`
/// and `THREAD_TOOLS_URL` are set: by every role, because the A2A adapter of a worker mints the
/// grants the control plane's endpoint verifies.
#[cfg(feature = "surface-thread-tools")]
#[derive(Clone, Debug)]
pub struct ThreadToolsSettings {
    /// The keys (`THREAD_TOOLS_SECRET`, and `THREAD_TOOLS_SECRET_PREVIOUS` for a rotation), the
    /// base URL agents reach the orchestrator at (`THREAD_TOOLS_URL`) and the lifetime of a
    /// token (`THREAD_TOOLS_TOKEN_TTL_SECS`). The A2A adapter mints with it, and the endpoint
    /// verifies with its keys. Its `Debug` shows key ids, never a key.
    pub issuer: Arc<ThreadToolsIssuer>,
    /// `THREAD_TOOLS_ALLOWED_HOSTS`: the `Host` values the endpoint accepts; by default the host
    /// (and port, when the URL names one) of `THREAD_TOOLS_URL`.
    pub allowed_hosts: Vec<String>,
}

/// One entry of `MCP_TOKENS_FILE`, as written.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TokenSpec {
    user: String,
    token_env: String,
    /// The role the token has (ADR 0033), one of `auth.roles`; none: the default role.
    #[serde(default)]
    role: Option<String>,
}

/// The `transport` key of an `AGENTS_FILE` entry: how the orchestrator reaches the agent.
/// Absent means `a2a`, so every existing file stays valid. A value that is not listed here is a
/// parse error naming the choices.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum TransportKind {
    /// A remote A2A agent: `cardUrl` (required), optional `tokenEnv`.
    #[default]
    A2a,
    /// An agent hosted in this process: `agent` (required, a [`LocalAgentKind`]). It has no card
    /// URL and no token, so `cardUrl` and `tokenEnv` are refused rather than ignored.
    Local,
}

/// One entry of `AGENTS_FILE`, as written.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentSpec {
    id: String,
    name: String,
    #[serde(default)]
    transport: TransportKind,
    /// Required for `a2a`, refused for `local`.
    card_url: Option<String>,
    /// `a2a` only.
    token_env: Option<String>,
    /// Required for `local` (the kind of agent), refused for `a2a`.
    agent: Option<String>,
    /// The verification gate of this target (ADR 0018), on top of the deployment's.
    gate: Option<GateLayer>,
}

/// A kind of agent hosted in the orchestrator's own process (`transport: local`, ADR 0015).
///
/// A closed enum (ADR 0004), defined here so the configuration can name and validate the kinds
/// without depending on any agent implementation crate; the composition root maps a kind to its
/// implementation. The endpoint carries only the kind's [`name`](Self::name)
/// (`AgentTransport::Local`), so the ports know no kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalAgentKind {
    /// Repeats the user's message back. For tests and demos; needs no model.
    Echo,
}

impl LocalAgentKind {
    /// Every kind this source tree knows, compiled in or not.
    pub const ALL: &'static [LocalAgentKind] = &[LocalAgentKind::Echo];

    /// The name used as `agent:` in `AGENTS_FILE`, and as the endpoint's `AgentTransport::Local`
    /// name.
    pub const fn name(self) -> &'static str {
        match self {
            LocalAgentKind::Echo => "echo",
        }
    }

    /// The Cargo feature of the `orchestrator` package that compiles local agents in.
    pub const fn feature(self) -> &'static str {
        match self {
            LocalAgentKind::Echo => "agent-local",
        }
    }

    /// Whether this build contains the kind: the Cargo feature `agent-local` compiles in
    /// `orch-agent-adam`, which hosts every kind listed here.
    pub const fn compiled_in(self) -> bool {
        match self {
            LocalAgentKind::Echo => cfg!(feature = "agent-local"),
        }
    }

    /// Whether the kind calls a language model, and so needs the model endpoint configured
    /// (ADR 0005). Echo needs none; a kind that does adds a check at startup, not at the first
    /// message.
    #[cfg_attr(
        not(feature = "agent-local"),
        allow(dead_code, reason = "only the composition of local agents asks")
    )]
    pub const fn needs_model(self) -> bool {
        match self {
            LocalAgentKind::Echo => false,
        }
    }

    /// One line for humans, shown when a name is not recognised.
    pub const fn description(self) -> &'static str {
        match self {
            LocalAgentKind::Echo => "repeats the user's message back (tests and demos)",
        }
    }

    fn known() -> String {
        Self::ALL
            .iter()
            .map(|k| format!("{} ({})", k.name(), k.description()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.name() == name)
    }
}

/// Log output format (`LOG_FORMAT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line (the default; what a log collector wants).
    Json,
    /// Human-readable lines for local development.
    Text,
}

impl LogFormat {
    /// Parses `LOG_FORMAT`; anything but `text` is JSON.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("text") => LogFormat::Text,
            _ => LogFormat::Json,
        }
    }
}

/// An interaction surface: a set of HTTP routes over `App` that `ORCH_SURFACES` mounts.
///
/// A closed enum (ADR 0004). The resource API and health are not surfaces: they are always
/// mounted. Each surface is an adapter crate behind a Cargo feature of this binary (ADR 0009):
/// the feature decides what *can* be mounted, `ORCH_SURFACES` what *is*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The AG-UI routes (`orch-surface-agui`: run, connect, capabilities): the default user-facing
    /// protocol (ADR 0012).
    Agui,
    /// The MCP server (`orch-surface-mcp`: `/mcp`, a machine route guarded by bearer tokens):
    /// Claude Code, opencode or any MCP client starts and follows jobs (ADR 0019).
    Mcp,
    /// The per-thread MCP endpoint (`orch-surface-thread-tools`: `/thread-tools/{threadId}/mcp`, a
    /// machine route guarded by the HMAC token the A2A adapter mints): the tools an agent that
    /// lists `thread-tools/v1` calls back (`thread-tools/v1`, ADR 0023).
    ThreadTools,
    /// `POST /webhooks/ci` (`orch-surface-webhook`): the generic signed CI report, a machine route
    /// outside the identity layer (ADR 0017).
    WebhookGeneric,
    /// `POST /webhooks/github` (`orch-surface-webhook`): GitHub's own `check_run` and
    /// `workflow_run` deliveries, a machine route guarded by `X-Hub-Signature-256` (ADR 0017).
    WebhookGithub,
}

/// A surface name that used to exist. Asking for one is a [`ConfigError::RemovedSurface`], not an
/// [`ConfigError::UnknownSurface`]: the operator learns what happened and what to use instead.
struct RemovedSurface {
    name: &'static str,
    removed: &'static str,
    what: &'static str,
    replacement: &'static str,
}

impl RemovedSurface {
    /// The removal `name` refers to, as the error the operator sees.
    fn refusing(name: &str) -> Option<ConfigError> {
        REMOVED
            .iter()
            .find(|r| r.name == name)
            .map(|gone| ConfigError::RemovedSurface {
                name: gone.name,
                removed: gone.removed,
                what: gone.what,
                replacement: gone.replacement,
            })
    }
}

/// The surfaces that were removed (ADR 0012), newest last.
const REMOVED: &[RemovedSurface] = &[RemovedSurface {
    name: "chat-api",
    removed: "2026-09-30",
    what: "the legacy chat API interaction routes (createThread, postMessage, listEvents, \
           streamEvents; POST /api/threads and /api/threads/{id}/messages, \
           GET /api/threads/{id}/events and /api/threads/{id}/stream)",
    replacement: "POST /agui/agents/{agentId}, GET /agui/threads/{threadId}/connect and \
                  GET /agui/agents/{agentId}/capabilities (docs/api/agui.md); the resource API \
                  (GET /api/threads, GET /api/threads/{id}, GET /api/agents, cancel) is unchanged",
}];

impl Surface {
    /// Every surface this source tree knows, compiled in or not.
    pub const ALL: &'static [Surface] = &[
        Surface::Agui,
        Surface::Mcp,
        Surface::ThreadTools,
        Surface::WebhookGeneric,
        Surface::WebhookGithub,
    ];

    /// The name used in `ORCH_SURFACES`.
    pub const fn name(self) -> &'static str {
        match self {
            Surface::Agui => "agui",
            Surface::Mcp => "mcp",
            Surface::ThreadTools => "thread-tools",
            Surface::WebhookGeneric => "webhook-generic",
            Surface::WebhookGithub => "webhook-github",
        }
    }

    /// The Cargo feature of the `orchestrator` package that compiles the surface in.
    pub const fn feature(self) -> &'static str {
        match self {
            Surface::Agui => "surface-agui",
            Surface::Mcp => "surface-mcp",
            Surface::ThreadTools => "surface-thread-tools",
            Surface::WebhookGeneric | Surface::WebhookGithub => "surface-webhook",
        }
    }

    /// Whether this build contains the surface.
    pub const fn compiled_in(self) -> bool {
        match self {
            Surface::Agui => cfg!(feature = "surface-agui"),
            Surface::Mcp => cfg!(feature = "surface-mcp"),
            Surface::ThreadTools => cfg!(feature = "surface-thread-tools"),
            Surface::WebhookGeneric | Surface::WebhookGithub => cfg!(feature = "surface-webhook"),
        }
    }

    fn known() -> String {
        Self::ALL
            .iter()
            .map(|s| s.name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn parse(name: &str) -> Result<Self, ConfigError> {
        let surface = Self::ALL
            .iter()
            .copied()
            .find(|s| s.name() == name)
            .ok_or_else(|| ConfigError::UnknownSurface {
                name: name.to_owned(),
                known: Self::known(),
            })?;
        if surface.compiled_in() {
            Ok(surface)
        } else {
            Err(ConfigError::SurfaceNotCompiled {
                surface: surface.name(),
                feature: surface.feature(),
            })
        }
    }
}

impl fmt::Display for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The surfaces mounted when `ORCH_SURFACES` is not set, as far as this build contains them
/// (a build without a surface's feature simply does not serve it by default, whereas *asking*
/// for it by name is an error). The list moved with the AG-UI migration (ADR 0012): `chat-api`
/// alone, then `agui,chat-api` while the web was migrated, then `agui` alone; the `chat-api`
/// surface was removed on 2026-09-30, and naming it is an error ([`REMOVED`]).
fn default_surfaces() -> Vec<Surface> {
    [Surface::Agui]
        .into_iter()
        .filter(|s| s.compiled_in())
        .collect()
}

/// Parses `ORCH_SURFACES`: a comma-separated list of surface names, at least one, no repeats,
/// each known and compiled in. Whitespace around a name is ignored.
fn parse_surfaces(raw: &str) -> Result<Vec<Surface>, ConfigError> {
    let invalid = |reason: String| ConfigError::Invalid {
        var: "ORCH_SURFACES",
        reason,
    };
    let names: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return Err(invalid(format!(
            "the list is empty; name at least one of {}",
            Surface::known()
        )));
    }
    // A removed surface is reported whatever else the list holds, and before any other name is
    // judged: `agui,chat-api` (the old default) says what happened, in every build.
    if let Some(err) = names.iter().find_map(|name| RemovedSurface::refusing(name)) {
        return Err(err);
    }
    let mut surfaces: Vec<Surface> = Vec::with_capacity(names.len());
    for name in names {
        let surface = Surface::parse(name)?;
        if surfaces.contains(&surface) {
            return Err(invalid(format!("{name:?} is listed more than once")));
        }
        surfaces.push(surface);
    }
    Ok(surfaces)
}

/// The command line. Every flag falls back to the environment variable shown in `--help`.
///
/// The values are raw strings on purpose: [`Config::load`] validates them, so a bad value is
/// one [`ConfigError`] naming the variable, whichever way it was supplied.
#[derive(Debug, Default, Parser)]
#[command(
    name = "orchestrator",
    version,
    about = "The orchestration layer: chat surfaces over a Postgres event log and durable A2A delegation.",
    after_help = "Every option can also be set through the environment variable shown as \
[env: NAME]. A flag wins over its variable. An empty value counts as unset. The settings are \
better kept in the configuration file (--config, ORCH_CONFIG_FILE); a flag or variable that is \
set wins over it, and is deprecated (ORCH_ROLE, ORCH_INSTANCE_ID, RUST_LOG and HOSTNAME stay). The bearer token \
of an agent is read from the variable its `tokenEnv` names in AGENTS_FILE, never from a flag. \
Logging is filtered by RUST_LOG (default: info)."
)]
pub struct Args {
    /// The configuration file (YAML, `version: 1`; docs/api/config.md): every setting below as a
    /// key, secrets by reference, read once at startup. A variable or flag that is set wins over
    /// the file and is logged as a warning naming the key; the variables are deprecated. Unset:
    /// the environment alone configures the process, as before.
    #[arg(long = "config", env = "ORCH_CONFIG_FILE", value_name = "PATH")]
    pub config: Option<String>,

    /// Print the configuration this process would run with (the file, the variables over it, the
    /// defaults filled in; secrets as their references, never their values) and exit 0, or list
    /// the errors and exit 78. Opens no connection.
    #[arg(long)]
    pub print_config: bool,

    /// Postgres connection string (required). Never logged.
    #[arg(long, env = "DATABASE_URL", value_name = "URL", hide_env_values = true)]
    pub database_url: Option<String>,

    /// YAML list of `{id, name, transport?, cardUrl?, tokenEnv?, agent?, gate?}`: the agents of
    /// this deployment, listed first. Required unless AGENT_REGISTRY_URL is set, and then it may
    /// be unset or list no agent.
    #[arg(long, env = "AGENTS_FILE", value_name = "PATH")]
    pub agents_file: Option<String>,

    /// The platform's agent registry (`agent-registry/v1`, ADR 0022): the full URL of the
    /// document, `http` or `https`, without a user name or password
    /// (`https://platform.example.com/registry/v1/agents`). The agents it lists are read live,
    /// beside the agents of AGENTS_FILE (which come first and win on an id both list), and a
    /// registry that cannot be read leaves only those. Unset (the default) means no registry.
    /// Needs the Cargo feature `registry-platform` (exit 78 without it).
    #[arg(long, env = "AGENT_REGISTRY_URL", value_name = "URL")]
    pub registry_url: Option<String>,

    /// The bearer token sent to the registry, when it wants one. Never logged.
    #[arg(
        long,
        env = "AGENT_REGISTRY_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true
    )]
    pub registry_token: Option<String>,

    /// The bearer token sent to every agent the registry lists: one deployment-wide credential
    /// until authentication to the agents is decided (open question 11). Never logged.
    #[arg(
        long,
        env = "AGENT_REGISTRY_AGENT_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true
    )]
    pub registry_agent_token: Option<String>,

    /// Seconds one read of the registry may take, 1 to 60 (default 3). A registry that is slower
    /// is treated as down: its agents are not listed.
    #[arg(long, env = "AGENT_REGISTRY_TIMEOUT_SECS", value_name = "SECS")]
    pub registry_timeout_secs: Option<String>,

    /// The most seconds a copy of the registry document is kept before it is read again, 1 to
    /// 3600 (default 60), whatever the registry's own `Cache-Control` allows. The copy is kept in
    /// this process only.
    #[arg(long, env = "AGENT_REGISTRY_MAX_AGE_SECS", value_name = "SECS")]
    pub registry_max_age_secs: Option<String>,

    /// Address to listen on (default 0.0.0.0:8080).
    #[arg(long, env = "LISTEN_ADDR", value_name = "ADDR")]
    pub listen_addr: Option<String>,

    /// What this process runs: `all` (the default), `control-plane` (the HTTP server, the
    /// resource API and the surfaces; no dispatcher) or `worker` (the dispatcher, and a router
    /// with only /healthz and /readyz on LISTEN_ADDR). Migrations run in every role.
    #[arg(long, env = "ORCH_ROLE", value_name = "ROLE")]
    pub role: Option<String>,

    /// Interaction surfaces to mount, comma separated (default agui, as far as the build has
    /// it). Known: agui, mcp (the MCP server at /mcp; it needs MCP_TOKENS_FILE and
    /// MCP_ALLOWED_HOSTS), thread-tools (the per-thread MCP endpoint at
    /// /thread-tools/{threadId}/mcp; it needs THREAD_TOOLS_SECRET and THREAD_TOOLS_URL, which every
    /// role should get), webhook-generic (POST /webhooks/ci; needs WEBHOOK_GENERIC_SECRETS),
    /// webhook-github (POST /webhooks/github; needs WEBHOOK_GITHUB_SECRETS). The webhooks are
    /// machine routes with no user identity, guarded by a signature. The removed legacy `chat-api` is refused at
    /// startup. The resource API and health are always mounted.
    #[arg(long, env = "ORCH_SURFACES", value_name = "LIST")]
    pub surfaces: Option<String>,

    /// Sources every job's work must pass before it is done, comma separated: ci, agent-checks,
    /// verifier (default none: an agent that completes is done). This build honours ci, agent-checks
    /// and verifier. A ci gate needs reports: mount webhook-generic in ORCH_SURFACES. `verifier`
    /// needs ORCH_VERIFIER (or a `gate.verifier` in AGENTS_FILE).
    #[arg(long, env = "ORCH_GATE", value_name = "LIST")]
    pub gate: Option<String>,

    /// Attempts a job's agent gets under a gate, the first included, at least 1 (default 3).
    #[arg(long, env = "ORCH_MAX_ATTEMPTS", value_name = "N")]
    pub max_attempts: Option<String>,

    /// The most a target or a thread may raise the attempts to, at least 1 (default 10).
    #[arg(long, env = "ORCH_MAX_ATTEMPTS_CAP", value_name = "N")]
    pub max_attempts_cap: Option<String>,

    /// The agent that verifies the pushed work (an id in AGENTS_FILE; another agent than the
    /// one being verified). Needed when the gate requires `verifier`.
    #[arg(long, env = "ORCH_VERIFIER", value_name = "AGENT")]
    pub verifier: Option<String>,
    /// YAML list of `{user, tokenEnv}` for the surface `mcp` (required when it is mounted): the
    /// e-mail that owns the jobs of a token, and the environment variable that holds the bearer
    /// token. A variable that is unset stops the service. Read by the roles that serve HTTP.
    #[arg(long, env = "MCP_TOKENS_FILE", value_name = "PATH")]
    pub mcp_tokens_file: Option<String>,

    /// Host names the surface `mcp` accepts in the Host header, comma separated, for example
    /// `orch.example.com,orch.example.com:443` (required when it is mounted; a name without a
    /// port matches any port). Guards against DNS rebinding.
    #[arg(long, env = "MCP_ALLOWED_HOSTS", value_name = "LIST")]
    pub mcp_allowed_hosts: Option<String>,

    /// The public origin of the chat, for example `https://chat.example.com`. The surface `mcp`
    /// gives `start_job` a `web_url` from it; without it there is none.
    #[arg(long, env = "ORCH_PUBLIC_URL", value_name = "URL")]
    pub public_url: Option<String>,

    /// The largest `timeout_secs` the tool `wait_for_job` of the surface `mcp` honours, in
    /// seconds, between 1 and 86400 (default 3600). A larger request is cut to it.
    #[arg(long, env = "MCP_WAIT_MAX_SECS", value_name = "SECS")]
    pub mcp_wait_max_secs: Option<String>,

    /// The most `wait_for_job` calls this process holds open at once, at least 1 (default 256).
    /// Over it a call is refused ("too many waits").
    #[arg(long, env = "MCP_WAIT_MAX_CONCURRENT", value_name = "N")]
    pub mcp_wait_max_concurrent: Option<String>,

    /// The most `wait_for_job` calls one user may hold open at once, at least 1 (default 16).
    #[arg(long, env = "MCP_WAIT_MAX_PER_USER", value_name = "N")]
    pub mcp_wait_max_per_user: Option<String>,

    /// Browser origins the surface `mcp` lets through, comma separated, for example
    /// `https://inspector.example.com`. A request with an Origin header that is not listed is
    /// refused (403) whatever its token; requests without one, from every non-browser client,
    /// are not affected. Default: none.
    #[arg(long, env = "MCP_ALLOWED_ORIGINS", value_name = "LIST")]
    pub mcp_allowed_origins: Option<String>,

    /// The HMAC key of the thread-tools tokens, at least 32 bytes (`openssl rand -hex 32`). With
    /// THREAD_TOOLS_URL it makes the A2A adapter give a grant (the endpoint's address and a token)
    /// to every agent whose card lists the `thread-tools/v1` extension, and it opens the surface
    /// `thread-tools`; required (exit 78 otherwise) when ORCH_SURFACES mounts it. Give every role
    /// the same value: workers mint, the control plane serves. Never logged.
    #[arg(
        long,
        env = "THREAD_TOOLS_SECRET",
        value_name = "KEY",
        hide_env_values = true
    )]
    pub thread_tools_secret: Option<String>,

    /// The previous thread-tools key, for verifying only: a token signed with it is still
    /// accepted, so the key can be rotated without refusing the tokens in flight. Remove it once
    /// the longest token lifetime has passed. Never logged.
    #[arg(
        long,
        env = "THREAD_TOOLS_SECRET_PREVIOUS",
        value_name = "KEY",
        hide_env_values = true
    )]
    pub thread_tools_secret_previous: Option<String>,

    /// The base URL under which agents reach this orchestrator, `http` or `https`, for example
    /// `http://orchestrator:8080`. The grant tells an agent
    /// `<URL>/thread-tools/<threadId>/mcp`. Set with THREAD_TOOLS_SECRET or not at all.
    #[arg(long, env = "THREAD_TOOLS_URL", value_name = "URL")]
    pub thread_tools_url: Option<String>,

    /// Seconds a thread-tools token lives, between 60 and 86400 (default 7200). A task can run
    /// long, and a call that fails in the middle of one is hard for an agent to recover from.
    #[arg(long, env = "THREAD_TOOLS_TOKEN_TTL_SECS", value_name = "SECS")]
    pub thread_tools_token_ttl_secs: Option<String>,

    /// Host names the surface `thread-tools` accepts in the Host header, comma separated, for
    /// example `orchestrator:8080,localhost:8080` (default: the host, and port if the URL has one,
    /// of THREAD_TOOLS_URL; a name without a port matches any port). Guards against DNS rebinding.
    #[arg(long, env = "THREAD_TOOLS_ALLOWED_HOSTS", value_name = "LIST")]
    pub thread_tools_allowed_hosts: Option<String>,

    /// Seconds the verifier has to answer before the thread waits for the user, at least 1
    /// (default 1800). Waiting does not use an attempt.
    #[arg(long, env = "ORCH_VERIFIER_TIMEOUT_SECS", value_name = "SECS")]
    pub verifier_timeout_secs: Option<String>,

    /// Seconds between a verification's looks at its thread while it waits for the verifier, at
    /// least 1 (default 5). When the thread has moved on (a timeout, a cancel, a message from
    /// the user) the verifier is told to stop and the row ends, so a verifier that hangs does
    /// not hold a worker; this is how late that happens.
    #[arg(long, env = "ORCH_VERIFIER_WATCH_SECS", value_name = "SECS")]
    pub verifier_watch_secs: Option<String>,

    /// Whether a step's input and output (what a tool was called with and what it returned) are
    /// recorded in the log, redacted and capped (ADR 0030): `true` (the default) or `false`
    /// (also `1`/`0`, `yes`/`no`, `on`/`off`). Off, a step is its label and detail only, as
    /// before ADR 0030, and nothing the tools received or returned is kept.
    #[arg(long, env = "ORCH_STEPS_RECORD_IO", value_name = "BOOL")]
    pub steps_record_io: Option<String>,

    /// The model that writes thread titles (the orchestrator's own first model call: after the
    /// agent's first reply it is asked for a 3 to 6 word title, which replaces the first words of
    /// the first message unless a person renamed the thread). Unset (the default) turns titles
    /// off. Needs ORCH_MODEL_BASE_URL.
    #[arg(long, env = "ORCH_TITLE_MODEL", value_name = "MODEL")]
    pub title_model: Option<String>,

    /// The base URL of the OpenAI-compatible endpoint the title model is asked at, up to and not
    /// including `/chat/completions` (`https://api.openai.com/v1`); `http` or `https`. Required
    /// (exit 78 otherwise) when ORCH_TITLE_MODEL is set, and ignored without it.
    #[arg(long, env = "ORCH_MODEL_BASE_URL", value_name = "URL")]
    pub model_base_url: Option<String>,

    /// The bearer token the model endpoint wants, when it wants one. Never logged.
    #[arg(
        long,
        env = "ORCH_MODEL_API_KEY",
        value_name = "KEY",
        hide_env_values = true
    )]
    pub model_api_key: Option<String>,

    /// Seconds one question to the model may take, at least 1 (default 20). A model that does not
    /// answer in time costs the thread nothing: it keeps the first message's words.
    #[arg(long, env = "ORCH_MODEL_TIMEOUT_SECS", value_name = "SECS")]
    pub model_timeout_secs: Option<String>,
    /// Seconds a job waits for the CI reports its gate needs before it is blocked
    /// (`ci_timeout`; it does not use an attempt), at least 1 (default 3600). An agent's
    /// `gate.ci.timeoutSecs` in AGENTS_FILE overrides it.
    #[arg(long, env = "ORCH_CI_TIMEOUT_SECS", value_name = "SECS")]
    pub ci_timeout_secs: Option<String>,

    /// The names of the CI checks that must pass, comma separated: GitHub's check name
    /// (`check_run`) or workflow name (`workflow_run`), or the `name` of a generic report. A gate
    /// that requires `ci` must name at least one (here, or in an agent's `gate.ci.required`);
    /// without names the orchestrator refuses to start (78), because "the first report decides"
    /// would let a red commit pass on a `skipped` report.
    #[arg(long, env = "ORCH_CI_REQUIRED", value_name = "NAMES")]
    pub ci_required: Option<String>,

    /// One or two comma-separated shared secrets for POST /webhooks/ci: a report is accepted when
    /// its signature matches either, so a secret can be rotated without a gap. Required (exit 78
    /// otherwise) when ORCH_SURFACES mounts webhook-generic. Never logged.
    #[arg(
        long,
        env = "WEBHOOK_GENERIC_SECRETS",
        value_name = "SECRETS",
        hide_env_values = true
    )]
    pub webhook_generic_secrets: Option<String>,

    /// One or two comma-separated secrets of POST /webhooks/github (the secret of the GitHub
    /// webhook; X-Hub-Signature-256 by either is good, so a secret can be rotated). Required (exit
    /// 78 otherwise) when ORCH_SURFACES mounts webhook-github. Never logged.
    #[arg(
        long,
        env = "WEBHOOK_GITHUB_SECRETS",
        value_name = "SECRETS",
        hide_env_values = true
    )]
    pub webhook_github_secrets: Option<String>,

    /// Seconds old the signed completion time of a GitHub event may be, at least 1 (default 86400,
    /// one day). An older event is acknowledged (202) and not stored: a captured delivery is not
    /// replayable for ever.
    #[arg(long, env = "WEBHOOK_GITHUB_MAX_AGE_SECS", value_name = "SECS")]
    pub webhook_github_max_age_secs: Option<String>,

    /// Seconds the X-Vymalo-Timestamp of a generic webhook may differ from the clock, either
    /// way, at least 1 (default 300).
    #[arg(long, env = "WEBHOOK_GENERIC_MAX_SKEW_SECS", value_name = "SECS")]
    pub webhook_generic_max_skew_secs: Option<String>,

    /// E-mail served for requests without X-Auth-Request-Email. Development only.
    #[arg(long, env = "AUTH_DEV_USER", value_name = "EMAIL")]
    pub auth_dev_user: Option<String>,

    /// Pool size, at least 2 (default 10).
    #[arg(long, env = "DATABASE_MAX_CONNECTIONS", value_name = "N")]
    pub database_max_connections: Option<String>,

    /// Outbox rows processed at once, at least 1 (default 32).
    #[arg(long, env = "DISPATCHER_CONCURRENCY", value_name = "N")]
    pub dispatcher_concurrency: Option<String>,

    /// Runs of local agents (`transport: local`) stepped at once, at least 1 (default 4). The
    /// pool of the local agents is this plus 4 connections. Builds with the Cargo feature
    /// `agent-local` only.
    #[cfg(feature = "agent-local")]
    #[arg(long, env = "AGENT_LOCAL_CONCURRENCY", value_name = "N")]
    pub agent_local_concurrency: Option<String>,

    /// Seconds a crashed replica's claim blocks others, at least 3 (default 30). Also how long
    /// a crashed replica's local agent runs stay leased.
    #[arg(long, env = "OUTBOX_LEASE_SECS", value_name = "SECS")]
    pub outbox_lease_secs: Option<String>,

    /// Seconds an inbox worker's claim on a row lasts, at least 3 (default 30).
    #[arg(long, env = "INBOX_LEASE_SECS", value_name = "SECS")]
    pub inbox_lease_secs: Option<String>,

    /// Seconds between the inbox worker's polls, at least 1 (default 2). A timer fires at most
    /// this late.
    #[arg(long, env = "INBOX_POLL_SECS", value_name = "SECS")]
    pub inbox_poll_secs: Option<String>,

    /// Seconds a report that matches no watch waits before it expires, at least 1 (default
    /// 86400).
    #[arg(long, env = "INBOX_PARKED_TTL_SECS", value_name = "SECS")]
    pub inbox_parked_ttl_secs: Option<String>,

    /// Claims of one inbox row before it is given up on, at least 1 (default 10).
    #[arg(long, env = "INBOX_MAX_ATTEMPTS", value_name = "N")]
    pub inbox_max_attempts: Option<String>,

    /// Seconds a graceful shutdown may take, at least 1 (default 15).
    #[arg(long, env = "SHUTDOWN_GRACE_SECS", value_name = "SECS")]
    pub shutdown_grace_secs: Option<String>,

    /// Names this replica in outbox leases (default $HOSTNAME-<uuid>).
    #[arg(long, env = "ORCH_INSTANCE_ID", value_name = "ID")]
    pub instance_id: Option<String>,

    /// Log format, `json` or `text` (default json).
    #[arg(long, env = "LOG_FORMAT", value_name = "FORMAT")]
    pub log_format: Option<String>,

    /// The host name, the prefix of the default instance id.
    #[arg(long, env = "HOSTNAME", value_name = "NAME", hide = true)]
    pub hostname: Option<String>,
}

/// How requests are authenticated: `auth.mode` and `auth.jwt` (ADR 0033). Without a file, the
/// proxy header, as before the file existed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthSettings {
    /// `auth.mode`.
    pub mode: AuthMode,
    /// `auth.jwt`: set exactly when the mode reads tokens.
    pub jwt: Option<JwtSettings>,
    /// `auth.roles` and `auth.defaultRole`: what each role grants (the built-in `user` and `admin`
    /// without a file, as before roles existed everyone is a `user`).
    pub policy: Policy,
}

/// `auth.jwt`: the token issuer and what is read from its tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtSettings {
    /// `auth.jwt.issuer`.
    pub issuer: String,
    /// `auth.jwt.audiences`.
    pub audiences: Vec<String>,
    /// `auth.jwt.jwksUrl`.
    pub jwks_url: Option<String>,
    /// `auth.jwt.userClaim`.
    pub user_claim: String,
    /// `auth.jwt.rolesClaim`.
    pub roles_claim: Option<String>,
}

impl AuthSettings {
    /// The settings of a valid file's `auth` section.
    fn from_file(auth: &orch_config::Auth) -> Self {
        AuthSettings {
            mode: auth.mode,
            policy: policy_of(auth),
            jwt: auth.jwt.as_ref().map(|jwt| JwtSettings {
                issuer: jwt.issuer.trim().to_owned(),
                audiences: jwt.audiences.iter().map(|a| a.trim().to_owned()).collect(),
                jwks_url: jwt.jwks_url.as_ref().map(|u| u.trim().to_owned()),
                user_claim: jwt.user_claim.trim().to_owned(),
                roles_claim: jwt.roles_claim.as_ref().map(|c| c.trim().to_owned()),
            }),
        }
    }

    /// The Cargo feature this build lacks for the mode, when it lacks one.
    fn missing_feature(&self) -> Option<&'static str> {
        if self.mode.reads_tokens() && !cfg!(feature = "auth-jwt") {
            Some("auth-jwt")
        } else if self.mode.reads_header() && !cfg!(feature = "auth-header") {
            Some("auth-header")
        } else {
            None
        }
    }
}

/// The policy of a valid file's `auth` section (ADR 0033): its roles, or the built-in `user` and
/// `admin`; its default role, which is `user` only when no role is defined (a deployment that
/// defines its roles names the default, or has none: such a person is refused).
///
/// The file's rules have checked the default role against the roles, so the error arm is not a
/// path a running service takes; it is the policy that grants nothing.
fn policy_of(auth: &orch_config::Auth) -> Policy {
    let scope = |scope: orch_config::AuthScope| match scope {
        orch_config::AuthScope::Own => Scope::Own,
        orch_config::AuthScope::Any => Scope::Any,
    };
    let permission = |p: orch_config::AuthPermission| match p {
        orch_config::AuthPermission::AgentRead => Permission::AgentRead,
        orch_config::AuthPermission::AgentInvoke => Permission::AgentInvoke,
        orch_config::AuthPermission::ThreadRead => Permission::ThreadRead,
        orch_config::AuthPermission::ThreadWrite => Permission::ThreadWrite,
        orch_config::AuthPermission::ArtifactRead => Permission::ArtifactRead,
        orch_config::AuthPermission::Admin => Permission::Admin,
    };
    let (roles, built_in_default) = match &auth.roles {
        None => (built_in_roles(), Some("user")),
        Some(roles) => (
            roles
                .iter()
                .map(|(name, role)| {
                    let (read, write) =
                        role.scope
                            .as_ref()
                            .map_or((Scope::Own, Scope::Own), |scopes| {
                                let (read, write) = scopes.read_write();
                                (scope(read), scope(write))
                            });
                    let grant = RoleGrant {
                        permissions: role.permissions.iter().copied().map(permission).collect(),
                        read,
                        write,
                        agents: role
                            .agents
                            .as_ref()
                            .map_or(AgentScope::All, AgentScope::from_patterns),
                    };
                    (orch_ports::Role::new(name.as_str()), grant)
                })
                .collect(),
            None,
        ),
    };
    let default_role = match &auth.default_role {
        None => built_in_default.map(str::to_owned),
        Some(None) => None,
        Some(Some(name)) => Some(name.trim().to_owned()),
    };
    Policy::new(roles, default_role.map(orch_ports::Role::new)).unwrap_or_else(|e| {
        tracing::error!(error = %e, "auth.defaultRole is not one of auth.roles; no role grants anything");
        Policy::deny_all()
    })
}

/// The complete, validated configuration.
pub struct Config {
    /// `DATABASE_URL`. May contain a password: never logged.
    pub database_url: String,
    /// `LISTEN_ADDR`.
    pub listen_addr: SocketAddr,
    /// `AGENTS_FILE`, resolved: bearer tokens already read from their environment variables.
    /// Empty only when `AGENT_REGISTRY_URL` is set and the file is unset or lists none.
    pub agents: Vec<AgentEntry>,
    /// `AGENT_REGISTRY_URL` and the variables that go with it: the platform's agent registry,
    /// read beside `agents`. `None` when the URL is unset. Redacted in `Debug`.
    #[cfg(feature = "registry-platform")]
    pub registry: Option<RegistrySettings>,
    /// The gate new threads start under before an agent's entry or a request changes it:
    /// `ORCH_GATE`, `ORCH_MAX_ATTEMPTS`, `ORCH_VERIFIER`, `ORCH_VERIFIER_TIMEOUT_SECS`.
    pub gate: GatePolicy,
    /// What a target or a thread may ask of the gate: this build's sources and
    /// `ORCH_MAX_ATTEMPTS_CAP`.
    pub gate_rules: GateRules,
    /// The `gate` key of each `AGENTS_FILE` entry that has one.
    pub target_gates: BTreeMap<AgentId, GateLayer>,
    /// `ORCH_ROLE`: which halves this process runs (`adam-host`'s closed enum; default `all`).
    pub role: Role,
    /// `ORCH_SURFACES`: the interaction surfaces to mount, no repeats. Empty only when the
    /// variable is unset in a build that contains no default surface.
    pub surfaces: Vec<Surface>,
    /// `MCP_TOKENS_FILE`, `MCP_ALLOWED_HOSTS` and `ORCH_PUBLIC_URL`: set when the surface `mcp` is
    /// mounted by a role that serves HTTP.
    pub mcp: Option<McpSettings>,
    /// `THREAD_TOOLS_SECRET`, `THREAD_TOOLS_URL` and the variables that go with them: set when
    /// both are (in every role; refused when only one is, and when the surface `thread-tools` is
    /// named without them). Its `Debug` shows key ids, never a key.
    #[cfg(feature = "surface-thread-tools")]
    pub thread_tools: Option<ThreadToolsSettings>,
    /// `WEBHOOK_GENERIC_SECRETS` and `WEBHOOK_GENERIC_MAX_SKEW_SECS`: the generic webhook route
    /// (`None` when its secrets are unset, which is refused only if the route is to be mounted).
    /// Its `Debug` shows how many secrets there are, never their values.
    #[cfg(feature = "surface-webhook")]
    pub webhook_generic: Option<GenericConfig>,
    /// `WEBHOOK_GITHUB_SECRETS`: the GitHub webhook route (`None` when unset, which is refused only
    /// if the route is to be mounted). Redacted in `Debug` like the generic one.
    #[cfg(feature = "surface-webhook")]
    pub webhook_github: Option<GithubConfig>,
    /// `AUTH_DEV_USER`: an identity for requests without `X-Auth-Request-Email`. Dev only, and only
    /// with `auth.mode: proxy_header`.
    pub auth_dev_user: Option<UserId>,
    /// `auth.mode` and `auth.jwt`: how requests are authenticated (the proxy header, without a file).
    pub auth: AuthSettings,
    /// `DATABASE_MAX_CONNECTIONS` (at least 2: the wakeup listener holds one).
    pub database_max_connections: u32,
    /// `DISPATCHER_CONCURRENCY`: outbox rows processed at once.
    pub dispatcher_concurrency: usize,
    /// `AGENT_LOCAL_CONCURRENCY`: runs of local agents stepped at once.
    #[cfg(feature = "agent-local")]
    pub agent_local_concurrency: usize,
    /// `OUTBOX_LEASE_SECS`: how long a crashed replica's claim blocks others.
    pub outbox_lease: Duration,
    /// `ORCH_VERIFIER_WATCH_SECS`: how often a verification looks at its thread while it waits.
    pub verifier_watch: Duration,
    /// `ORCH_STEPS_RECORD_IO`: whether a step's input and output are recorded (ADR 0030).
    pub steps_record_io: bool,
    /// `models.endpoints` and `tasks` of the configuration file, or, from the environment alone,
    /// `ORCH_TITLE_MODEL`, `ORCH_MODEL_BASE_URL`, `ORCH_MODEL_API_KEY` and
    /// `ORCH_MODEL_TIMEOUT_SECS` (the endpoint `default` and the title task): the models the
    /// orchestrator asks itself. No task turns that task off. Its `Debug` never shows a key.
    pub models: ModelsSettings,
    /// `ui` of the configuration file: what `GET /api/config` says to the web.
    pub public: PublicConfig,
    /// `toolServers` of the configuration file: the MCP servers a person may attach to a
    /// conversation (ADR 0024), the public part of each. Empty without the key (and always from
    /// the environment alone: the section has no variable). The URL and the credentials are not
    /// here.
    pub tool_servers: Vec<ToolServerInfo>,
    /// `toolServers` of the configuration file, the part the relay needs to call them (ADR 0024): in
    /// the order of the file, each server's id, URL, timeout and resolved credentials as the port's
    /// endpoint type. Only the relay reads it (composed by the binary behind the feature `tool-relay`). Its `Debug`
    /// lists the ids and nothing else.
    pub tool_endpoints: Vec<ToolServerEndpoint>,
    /// `INBOX_LEASE_SECS`, `INBOX_POLL_SECS`, `INBOX_PARKED_TTL_SECS`, `INBOX_MAX_ATTEMPTS`: the
    /// inbox worker (timers and reports).
    pub inbox: InboxConfig,
    /// `ORCH_INSTANCE_ID`: names this replica in outbox leases.
    pub instance_id: String,
    /// `SHUTDOWN_GRACE_SECS`: how long a graceful shutdown may take.
    pub shutdown_grace: Duration,
    /// `LOG_FORMAT` (`log.format`): how a log line is written.
    pub log_format: LogFormat,
    /// `artifacts` of the configuration file: the store the files agents hand over are kept in,
    /// and the largest file kept (ADR 0032). `None` without the section (and always without a
    /// file: the section has no variable): no store, and a file is refused. Its `Debug` never
    /// shows a credential.
    pub artifacts: Option<ArtifactSettings>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Config");
        debug
            .field("database_url", &"<redacted>")
            .field("listen_addr", &self.listen_addr)
            .field("agents", &self.agents)
            .field("gate", &self.gate)
            .field("gate_rules", &self.gate_rules)
            .field("target_gates", &self.target_gates)
            .field("role", &self.role)
            .field("surfaces", &self.surfaces)
            .field("mcp", &self.mcp)
            .field("auth_dev_user", &self.auth_dev_user)
            .field("auth", &self.auth)
            .field("database_max_connections", &self.database_max_connections)
            .field("dispatcher_concurrency", &self.dispatcher_concurrency)
            .field("outbox_lease", &self.outbox_lease)
            .field("verifier_watch", &self.verifier_watch)
            .field("steps_record_io", &self.steps_record_io)
            .field("models", &self.models)
            .field("public", &self.public)
            .field("tool_servers", &self.tool_servers)
            .field(
                "tool_endpoints",
                &self
                    .tool_endpoints
                    .iter()
                    .map(|e| &e.id)
                    .collect::<Vec<_>>(),
            )
            .field("inbox", &self.inbox)
            .field("instance_id", &self.instance_id)
            .field("shutdown_grace", &self.shutdown_grace)
            .field("log_format", &self.log_format)
            .field("artifacts", &self.artifacts);
        #[cfg(feature = "agent-local")]
        debug.field("agent_local_concurrency", &self.agent_local_concurrency);
        #[cfg(feature = "registry-platform")]
        debug.field("registry", &self.registry);
        #[cfg(feature = "surface-thread-tools")]
        debug.field("thread_tools", &self.thread_tools);
        #[cfg(feature = "surface-webhook")]
        debug
            .field("webhook_generic", &self.webhook_generic)
            .field("webhook_github", &self.webhook_github);
        debug.finish()
    }
}

impl Config {
    /// Validates `args` against the process environment (for `tokenEnv`) and the real file
    /// system.
    pub fn from_args(args: Args) -> Result<Self, ConfigError> {
        Self::load(
            args,
            |name| std::env::var(name).ok(),
            |path| std::fs::read_to_string(path),
        )
    }

    /// Validates `args`. `env` looks up the variables named by agents' `tokenEnv`, and `read`
    /// returns the contents of `AGENTS_FILE`. An empty (or all-whitespace) value counts as unset.
    pub fn load(
        args: Args,
        env: impl Fn(&str) -> Option<String>,
        read: impl Fn(&Path) -> io::Result<String>,
    ) -> Result<Self, ConfigError> {
        Self::load_with(args, Resolved::default(), env, read)
    }

    /// [`load`](Self::load), with the values that cannot be handed over as one string each.
    fn load_with(
        args: Args,
        resolved: Resolved,
        env: impl Fn(&str) -> Option<String>,
        read: impl Fn(&Path) -> io::Result<String>,
    ) -> Result<Self, ConfigError> {
        #[cfg(not(feature = "surface-webhook"))]
        let _ = &resolved;
        let clean = |v: Option<String>| v.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let get_env = |name: &str| clean(env(name));
        let log_format = LogFormat::parse(args.log_format.as_deref());

        let database_url = clean(args.database_url).ok_or(ConfigError::Missing("DATABASE_URL"))?;
        let listen_addr = clean(args.listen_addr).unwrap_or_else(|| DEFAULT_LISTEN_ADDR.to_owned());
        let listen_addr = listen_addr
            .parse::<SocketAddr>()
            .map_err(|e| ConfigError::Invalid {
                var: "LISTEN_ADDR",
                reason: format!("{listen_addr:?} is not a socket address like 0.0.0.0:8080 ({e})"),
            })?;

        let registry_vars = RegistryVars {
            url: clean(args.registry_url),
            #[cfg(feature = "registry-platform")]
            token: clean(args.registry_token),
            #[cfg(feature = "registry-platform")]
            agent_token: clean(args.registry_agent_token),
            timeout_secs: clean(args.registry_timeout_secs),
            max_age_secs: clean(args.registry_max_age_secs),
        };
        // The registry's own variables are checked whether or not the URL is set (a bad number
        // is a typo to hear about now), and a URL with no registry in this build is refused.
        #[cfg(feature = "registry-platform")]
        let registry = registry_settings(registry_vars)?;
        #[cfg(not(feature = "registry-platform"))]
        let registry_url_set = refuse_registry(registry_vars)?;
        #[cfg(feature = "registry-platform")]
        let registry_url_set = registry.is_some();
        // Without a registry the agents are the file's, which must list some. With one, the file
        // may be unset or empty: the registry is where the agents come from.
        let (agents, target_gates) = match clean(args.agents_file) {
            Some(path) => {
                let agents_file = PathBuf::from(path);
                let text = read(&agents_file).map_err(|source| ConfigError::AgentsFileRead {
                    path: agents_file.clone(),
                    source,
                })?;
                parse_agents_full(
                    &text,
                    &agents_file,
                    get_env,
                    LocalAgentKind::compiled_in,
                    registry_url_set,
                )?
            }
            None if registry_url_set => (Vec::new(), BTreeMap::new()),
            None => return Err(ConfigError::Missing("AGENTS_FILE")),
        };
        // Blank is unset, so `all`. The enum and its names belong to adam-host; a name it does
        // not know is refused here, so the error is the usual one, naming the variable (78).
        let role =
            Role::from_optional(clean(args.role).as_deref()).map_err(|e| ConfigError::Invalid {
                var: "ORCH_ROLE",
                reason: e.to_string(),
            })?;

        // A value that is set but names no surface (`,`) is refused, so a typo cannot silently
        // fall back to the default. Blank (`""`) is unset, like every other variable.
        let surfaces = match clean(args.surfaces) {
            None => default_surfaces(),
            Some(raw) => parse_surfaces(&raw)?,
        };

        // A process that serves routes and mounts no CI webhook cannot receive a CI report, so a
        // job could only wait for one until its deadline: `ci` is refused, in every layer and per
        // thread. A worker serves no routes and cannot tell (the control plane decides).
        let ci_refusal = (role.runs_control_plane()
            && !surfaces
                .iter()
                .any(|s| matches!(s, Surface::WebhookGeneric | Surface::WebhookGithub)))
        .then_some(NO_CI_SURFACE);
        let (gate, gate_rules) = parse_gate(
            GateVars {
                gate: clean(args.gate),
                max_attempts: clean(args.max_attempts),
                max_attempts_cap: clean(args.max_attempts_cap),
                verifier: clean(args.verifier),
                verifier_timeout_secs: clean(args.verifier_timeout_secs),
                ci_timeout: clean(args.ci_timeout_secs),
                ci_required: clean(args.ci_required),
                ci_refusal,
            },
            &agents,
            &target_gates,
        )?;

        // The MCP surface needs its tokens and hosts, and only a role that serves HTTP mounts it:
        // a worker is not asked for secrets it would never use.
        let public_url = clean(args.public_url)
            .map(|raw| parse_public_url(&raw))
            .transpose()?;
        let mcp_wait_max_secs = number(
            clean(args.mcp_wait_max_secs),
            "MCP_WAIT_MAX_SECS",
            DEFAULT_MCP_WAIT_MAX_SECS,
            1,
        )?;
        let wait_max_concurrent = number(
            clean(args.mcp_wait_max_concurrent),
            "MCP_WAIT_MAX_CONCURRENT",
            DEFAULT_MCP_WAIT_MAX_CONCURRENT,
            1,
        )?;
        let wait_max_per_user = number(
            clean(args.mcp_wait_max_per_user),
            "MCP_WAIT_MAX_PER_USER",
            DEFAULT_MCP_WAIT_MAX_PER_USER,
            1,
        )?;
        let allowed_origins = parse_allowed_origins(clean(args.mcp_allowed_origins))?;
        if mcp_wait_max_secs > MAX_MCP_WAIT_MAX_SECS {
            return Err(ConfigError::Invalid {
                var: "MCP_WAIT_MAX_SECS",
                reason: format!("must be at most {MAX_MCP_WAIT_MAX_SECS}"),
            });
        }
        let mcp = if surfaces.contains(&Surface::Mcp) && role.runs_control_plane() {
            Some(McpSettings {
                tokens: parse_mcp_tokens(clean(args.mcp_tokens_file), &get_env, &read)?,
                allowed_hosts: parse_allowed_hosts(clean(args.mcp_allowed_hosts))?,
                public_url,
                wait_max: Duration::from_secs(mcp_wait_max_secs),
                wait_max_concurrent,
                wait_max_per_user,
                allowed_origins,
            })
        } else {
            None
        };
        // Every role reads these: the A2A adapter of a worker mints what the control plane's
        // endpoint verifies, so the surface being named is what makes them required, not the role.
        #[cfg(feature = "surface-thread-tools")]
        let thread_tools = thread_tools(
            ThreadToolsVars {
                secret: clean(args.thread_tools_secret),
                previous: clean(args.thread_tools_secret_previous),
                url: clean(args.thread_tools_url),
                ttl_secs: clean(args.thread_tools_token_ttl_secs),
                allowed_hosts: clean(args.thread_tools_allowed_hosts),
            },
            surfaces.contains(&Surface::ThreadTools),
        )?;
        #[cfg(feature = "surface-webhook")]
        let webhook_generic = webhook_generic(
            clean(args.webhook_generic_secrets)
                .map(WebhookSecrets::Joined)
                .or(resolved.webhook_generic.map(WebhookSecrets::Separate)),
            clean(args.webhook_generic_max_skew_secs),
            surfaces.contains(&Surface::WebhookGeneric) && role.runs_control_plane(),
        )?;

        #[cfg(feature = "surface-webhook")]
        let webhook_github = webhook_github(
            clean(args.webhook_github_secrets)
                .map(WebhookSecrets::Joined)
                .or(resolved.webhook_github.map(WebhookSecrets::Separate)),
            clean(args.webhook_github_max_age_secs),
            surfaces.contains(&Surface::WebhookGithub) && role.runs_control_plane(),
        )?;

        let auth_dev_user = match clean(args.auth_dev_user) {
            None => None,
            Some(email) if email.contains('@') => Some(UserId::new(&email)),
            Some(_) => {
                return Err(ConfigError::Invalid {
                    var: "AUTH_DEV_USER",
                    reason: "expected an e-mail address".to_owned(),
                });
            }
        };

        // The authenticator the mode needs is compiled in, or the process does not start (a
        // process that serves no routes authenticates nobody, and needs none).
        let auth = resolved.auth;
        if role.runs_control_plane()
            && let Some(feature) = auth.missing_feature()
        {
            return Err(ConfigError::AuthNotCompiled {
                mode: auth.mode.as_str(),
                feature,
            });
        }

        // A token's role is one the policy defines: a typo would otherwise hand its holder the
        // default role, which is not what the operator wrote (fail closed, ADR 0033).
        if let Some(mcp) = &mcp {
            for (user, roles, _) in &mcp.tokens {
                for role in roles {
                    if !auth.policy.role_names().any(|known| known == role) {
                        return Err(ConfigError::Invalid {
                            var: "MCP_TOKENS_FILE",
                            reason: format!(
                                "the role {:?} of {user} is not one of auth.roles ({})",
                                role.as_str(),
                                auth.policy
                                    .role_names()
                                    .map(|r| r.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        });
                    }
                }
            }
        }

        let database_max_connections = number(
            clean(args.database_max_connections),
            "DATABASE_MAX_CONNECTIONS",
            DEFAULT_DATABASE_MAX_CONNECTIONS,
            2,
        )?;
        let dispatcher_concurrency = number(
            clean(args.dispatcher_concurrency),
            "DISPATCHER_CONCURRENCY",
            DEFAULT_DISPATCHER_CONCURRENCY,
            1,
        )?;
        #[cfg(feature = "agent-local")]
        let agent_local_concurrency = number(
            clean(args.agent_local_concurrency),
            "AGENT_LOCAL_CONCURRENCY",
            DEFAULT_AGENT_LOCAL_CONCURRENCY,
            1,
        )?;
        let outbox_lease_secs = number(
            clean(args.outbox_lease_secs),
            "OUTBOX_LEASE_SECS",
            DEFAULT_OUTBOX_LEASE_SECS,
            3,
        )?;
        let verifier_watch_secs = number(
            clean(args.verifier_watch_secs),
            "ORCH_VERIFIER_WATCH_SECS",
            DEFAULT_VERIFIER_WATCH_SECS,
            1,
        )?;
        let steps_record_io = flag(clean(args.steps_record_io), "ORCH_STEPS_RECORD_IO", true)?;
        // The variables are read (and checked) whichever way the models are given, as they always
        // were; the file's own, when there is one, replace what they make.
        let from_variables = model_settings(
            clean(args.title_model),
            clean(args.model_base_url),
            clean(args.model_api_key),
            clean(args.model_timeout_secs),
        )?;
        let models = resolved.models.unwrap_or(from_variables);
        let public = resolved.public.unwrap_or_default();
        let tool_servers = resolved.tool_servers;
        let tool_endpoints = resolved.tool_endpoints;
        let inbox = InboxConfig {
            lease: Duration::from_secs(number(
                clean(args.inbox_lease_secs),
                "INBOX_LEASE_SECS",
                orch_app::DEFAULT_LEASE_SECS,
                3,
            )?),
            poll_interval: Duration::from_secs(number(
                clean(args.inbox_poll_secs),
                "INBOX_POLL_SECS",
                orch_app::DEFAULT_POLL_SECS,
                1,
            )?),
            parked_ttl: Duration::from_secs(number(
                clean(args.inbox_parked_ttl_secs),
                "INBOX_PARKED_TTL_SECS",
                orch_app::DEFAULT_PARKED_TTL_SECS,
                1,
            )?),
            max_attempts: number(
                clean(args.inbox_max_attempts),
                "INBOX_MAX_ATTEMPTS",
                orch_app::DEFAULT_MAX_ATTEMPTS,
                1,
            )?,
            ..InboxConfig::default()
        };
        let shutdown_grace_secs = number(
            clean(args.shutdown_grace_secs),
            "SHUTDOWN_GRACE_SECS",
            DEFAULT_SHUTDOWN_GRACE_SECS,
            1,
        )?;
        let instance_id = clean(args.instance_id).unwrap_or_else(|| {
            format!(
                "{}-{}",
                clean(args.hostname).unwrap_or_else(|| "orchestrator".to_owned()),
                uuid::Uuid::now_v7().simple()
            )
        });

        Ok(Config {
            database_url,
            listen_addr,
            agents,
            #[cfg(feature = "registry-platform")]
            registry,
            gate,
            gate_rules,
            target_gates,
            role,
            surfaces,
            mcp,
            #[cfg(feature = "surface-thread-tools")]
            thread_tools,
            #[cfg(feature = "surface-webhook")]
            webhook_generic,
            #[cfg(feature = "surface-webhook")]
            webhook_github,
            auth_dev_user,
            auth,
            database_max_connections,
            dispatcher_concurrency,
            #[cfg(feature = "agent-local")]
            agent_local_concurrency,
            outbox_lease: Duration::from_secs(outbox_lease_secs),
            verifier_watch: Duration::from_secs(verifier_watch_secs),
            steps_record_io,
            models,
            public,
            tool_servers,
            tool_endpoints,
            inbox,
            instance_id,
            shutdown_grace: Duration::from_secs(shutdown_grace_secs),
            log_format,
            artifacts: resolved.artifacts,
        })
    }
}

impl Config {
    /// The distinct kinds of local agent `AGENTS_FILE` lists, in file order. Empty when no entry
    /// has `transport: local`, and then no local runtime is built.
    #[cfg(feature = "agent-local")]
    pub fn local_kinds(&self) -> Vec<LocalAgentKind> {
        let mut kinds: Vec<LocalAgentKind> = Vec::new();
        for entry in &self.agents {
            if let orch_ports::AgentTransport::Local { name } = &entry.endpoint.transport
                && let Some(kind) = LocalAgentKind::parse(name)
                && !kinds.contains(&kind)
            {
                kinds.push(kind);
            }
        }
        kinds
    }
}

impl Config {
    /// The application settings the gate configuration contributes: what new threads start under
    /// and the rules requests are checked against.
    pub fn app_config(&self) -> AppConfig {
        let defaults = AppConfig::default();
        AppConfig {
            gate: self.gate.clone(),
            target_gates: self.target_gates.clone(),
            gate_rules: self.gate_rules.clone(),
            record_step_io: self.steps_record_io,
            tasks: self.models.tasks.clone(),
            public: self.public.clone(),
            policy: self.auth.policy.clone(),
            tool_servers: self.tool_servers.clone(),
            // The ingest's limits (ADR 0032); without an `artifacts` section there is no store and
            // they are never reached.
            files: self
                .artifacts
                .as_ref()
                .map_or(defaults.files, |a| orch_app::FileLimits {
                    max_file_bytes: a.max_file_bytes,
                    max_per_job_bytes: a.max_per_job_bytes,
                    ..orch_app::FileLimits::default()
                }),
            ..defaults
        }
    }
}

/// The models the orchestrator asks itself (ADR 0035): the endpoints it can reach, by name, and
/// the utility tasks that use them. Both are empty when no task is configured, which turns the
/// tasks off.
#[derive(Clone, Default)]
pub struct ModelsSettings {
    /// `models.endpoints`: name to endpoint.
    pub endpoints: BTreeMap<String, EndpointSettings>,
    /// `tasks`: how each task that is on asks its model. Every endpoint it names is in
    /// `endpoints` (the file's rules check it; the legacy variables make the one `default`).
    pub tasks: BTreeMap<TaskKind, TaskSettings>,
}

impl fmt::Debug for ModelsSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelsSettings")
            .field("endpoints", &self.endpoints)
            .field("tasks", &self.tasks)
            .finish()
    }
}

/// An OpenAI-compatible endpoint.
#[derive(Clone)]
pub struct EndpointSettings {
    /// The endpoint's base URL, `http` or `https`, without a trailing slash.
    pub base_url: String,
    /// The bearer token, when the endpoint wants one.
    pub api_key: Option<SecretString>,
    /// How long one question may take.
    pub timeout: Duration,
}

impl fmt::Debug for EndpointSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EndpointSettings")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// The models of the legacy variables: none when `ORCH_TITLE_MODEL` is unset (titles are off, and
/// the endpoint variables are not read, but a bad timeout is still refused), else the endpoint
/// named `default` that they make, which is then required, and the title task on it.
fn model_settings(
    title_model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    timeout_secs: Option<String>,
) -> Result<ModelsSettings, ConfigError> {
    let timeout = number(
        timeout_secs,
        "ORCH_MODEL_TIMEOUT_SECS",
        DEFAULT_MODEL_TIMEOUT_SECS,
        1,
    )?;
    let Some(title_model) = title_model else {
        return Ok(ModelsSettings::default());
    };
    let base_url = base_url.ok_or(ConfigError::Missing("ORCH_MODEL_BASE_URL"))?;
    let base_url = base_url.trim_end_matches('/').to_owned();
    let hosted = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .is_some_and(|rest| !rest.is_empty());
    if !hosted {
        return Err(ConfigError::Invalid {
            var: "ORCH_MODEL_BASE_URL",
            reason: "expected an http:// or https:// URL, like https://api.openai.com/v1"
                .to_owned(),
        });
    }
    let timeout = Duration::from_secs(timeout);
    Ok(ModelsSettings {
        endpoints: BTreeMap::from([(
            LEGACY_ENDPOINT.to_owned(),
            EndpointSettings {
                base_url,
                api_key: api_key.map(SecretString::from),
                timeout,
            },
        )]),
        tasks: BTreeMap::from([(
            TaskKind::Title,
            TaskSettings::new(TaskKind::Title, LEGACY_ENDPOINT, title_model).with_timeout(timeout),
        )]),
    })
}

/// The endpoint the legacy `ORCH_MODEL_*` variables make, and that `ORCH_TITLE_MODEL` asks.
const LEGACY_ENDPOINT: &str = "default";

/// The raw values of the registry's environment variables (blank already counted as unset).
struct RegistryVars {
    url: Option<String>,
    /// Only read by a build that has a registry.
    #[cfg(feature = "registry-platform")]
    token: Option<String>,
    #[cfg(feature = "registry-platform")]
    agent_token: Option<String>,
    timeout_secs: Option<String>,
    max_age_secs: Option<String>,
}

/// The platform's agent registry (`AGENT_REGISTRY_URL` and the variables that go with it).
#[cfg(feature = "registry-platform")]
#[derive(Clone)]
pub struct RegistrySettings {
    /// `AGENT_REGISTRY_URL`: an absolute `http(s)` URL with a host and no credentials.
    pub url: String,
    /// `AGENT_REGISTRY_TOKEN`: the bearer token for the registry, when it wants one.
    pub token: Option<SecretString>,
    /// `AGENT_REGISTRY_AGENT_TOKEN`: the bearer token sent to every agent the registry lists.
    pub agent_token: Option<SecretString>,
    /// `AGENT_REGISTRY_TIMEOUT_SECS`: how long one read may take.
    pub timeout: Duration,
    /// `AGENT_REGISTRY_MAX_AGE_SECS`: the longest a copy of the document is kept.
    pub max_age: Duration,
}

#[cfg(feature = "registry-platform")]
impl fmt::Debug for RegistrySettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The URL may carry a secret in its query: show where it goes, not what it says.
        let shown = url::Url::parse(&self.url)
            .map(|u| {
                let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
                format!(
                    "{}://{}{port}{}",
                    u.scheme(),
                    u.host_str().unwrap_or(""),
                    u.path()
                )
            })
            .unwrap_or_else(|_| "<not a URL>".to_owned());
        f.debug_struct("RegistrySettings")
            .field("url", &shown)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field(
                "agent_token",
                &self.agent_token.as_ref().map(|_| "<redacted>"),
            )
            .field("timeout", &self.timeout)
            .field("max_age", &self.max_age)
            .finish()
    }
}

/// A number of seconds in `min..=max`.
fn seconds_between(
    raw: Option<String>,
    var: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConfigError> {
    let value = number(raw, var, default, min)?;
    if value > max {
        return Err(ConfigError::Invalid {
            var,
            reason: format!("must be at most {max}"),
        });
    }
    Ok(value)
}

/// The registry's settings: none when `AGENT_REGISTRY_URL` is unset (the other variables are then
/// only checked for being numbers in range), else the URL, which must be an absolute `http(s)`
/// URL with a host and no user name or password.
#[cfg(feature = "registry-platform")]
fn registry_settings(vars: RegistryVars) -> Result<Option<RegistrySettings>, ConfigError> {
    let timeout = seconds_between(
        vars.timeout_secs,
        "AGENT_REGISTRY_TIMEOUT_SECS",
        DEFAULT_REGISTRY_TIMEOUT_SECS,
        1,
        MAX_REGISTRY_TIMEOUT_SECS,
    )?;
    let max_age = seconds_between(
        vars.max_age_secs,
        "AGENT_REGISTRY_MAX_AGE_SECS",
        DEFAULT_REGISTRY_MAX_AGE_SECS,
        1,
        MAX_REGISTRY_MAX_AGE_SECS,
    )?;
    let Some(url) = vars.url else {
        return Ok(None);
    };
    let bad = |reason: &str| ConfigError::Invalid {
        var: "AGENT_REGISTRY_URL",
        reason: reason.to_owned(),
    };
    let parsed = Url::parse(&url).map_err(|_| {
        bad("expected an absolute http:// or https:// URL, like https://platform.example.com/registry/v1/agents")
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.has_host() {
        return Err(bad(
            "expected an absolute http:// or https:// URL, like https://platform.example.com/registry/v1/agents",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(bad(
            "must not carry a user name or password (use AGENT_REGISTRY_TOKEN for a bearer token)",
        ));
    }
    Ok(Some(RegistrySettings {
        url,
        token: vars.token.map(SecretString::from),
        agent_token: vars.agent_token.map(SecretString::from),
        timeout: Duration::from_secs(timeout),
        max_age: Duration::from_secs(max_age),
    }))
}

/// A build without the platform registry: a URL for one is an error, and the numbers are still
/// checked. Whether a registry is configured is then always `false`.
#[cfg(not(feature = "registry-platform"))]
fn refuse_registry(vars: RegistryVars) -> Result<bool, ConfigError> {
    let RegistryVars {
        url,
        timeout_secs,
        max_age_secs,
    } = vars;
    seconds_between(
        timeout_secs,
        "AGENT_REGISTRY_TIMEOUT_SECS",
        DEFAULT_REGISTRY_TIMEOUT_SECS,
        1,
        MAX_REGISTRY_TIMEOUT_SECS,
    )?;
    seconds_between(
        max_age_secs,
        "AGENT_REGISTRY_MAX_AGE_SECS",
        DEFAULT_REGISTRY_MAX_AGE_SECS,
        1,
        MAX_REGISTRY_MAX_AGE_SECS,
    )?;
    if url.is_some() {
        return Err(ConfigError::RegistryNotCompiled);
    }
    Ok(false)
}

/// The raw values of the gate's environment variables (blank already counted as unset).
struct GateVars {
    gate: Option<String>,
    max_attempts: Option<String>,
    max_attempts_cap: Option<String>,
    verifier: Option<String>,
    verifier_timeout_secs: Option<String>,
    ci_timeout: Option<String>,
    ci_required: Option<String>,
    /// Why `ci` cannot be honoured in this process, when it cannot.
    ci_refusal: Option<&'static str>,
}

/// Why `ci` is refused when no CI webhook surface is mounted.
pub const NO_CI_SURFACE: &str = "no CI webhook surface is mounted (ORCH_SURFACES)";

fn gate_var(var: &'static str, e: impl fmt::Display) -> ConfigError {
    ConfigError::Invalid {
        var,
        reason: e.to_string(),
    }
}

/// The deployment's gate and the rules, from the environment, then every target's entry put on
/// top of it. Every refusal is a startup error (78): a gate that could not be honoured must not
/// quietly become no gate.
fn parse_gate(
    vars: GateVars,
    agents: &[AgentEntry],
    targets: &BTreeMap<AgentId, GateLayer>,
) -> Result<(GatePolicy, GateRules), ConfigError> {
    let cap = number(
        vars.max_attempts_cap,
        "ORCH_MAX_ATTEMPTS_CAP",
        DEFAULT_MAX_ATTEMPTS_CAP,
        1,
    )?;
    if cap > MAX_ATTEMPTS_CAP_CEILING {
        return Err(gate_var(
            "ORCH_MAX_ATTEMPTS_CAP",
            format!("{cap} is above the ceiling of {MAX_ATTEMPTS_CAP_CEILING}"),
        ));
    }
    let mut rules = GateRules::new(cap);
    if let Some(reason) = vars.ci_refusal {
        rules = rules.refusing(CheckSource::Ci, reason);
    }
    let at = Layer::Deployment;

    let mut policy = GatePolicy::default();
    if let Some(list) = vars.gate {
        let mut sources = Vec::new();
        for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let source = CheckSource::from_config_name(name).ok_or_else(|| {
                gate_var(
                    "ORCH_GATE",
                    format!("unknown source {name:?} (one of: {})", known_sources()),
                )
            })?;
            if let Some(refusal) = rules.refuse_source(&at, source) {
                return Err(gate_var("ORCH_GATE", refusal));
            }
            sources.push(source);
        }
        if sources.is_empty() {
            return Err(gate_var(
                "ORCH_GATE",
                format!("names no source (one of: {})", known_sources()),
            ));
        }
        policy.require = sources.into_iter().collect();
    }
    // How long a job waits for CI (ADR 0017). Read for every gate, not only one that requires
    // `ci`: an agent's own gate may require it, and its `ci.timeoutSecs` overrides this.
    let ci_timeout = number(
        vars.ci_timeout,
        "ORCH_CI_TIMEOUT_SECS",
        DEFAULT_CI_TIMEOUT_SECS,
        1,
    )?;
    policy.ci.timeout = jiff::SignedDuration::from_secs(ci_timeout);
    if let Some(list) = vars.ci_required {
        if let Some(refusal) = rules.refuse_ci_settings(&at) {
            return Err(gate_var("ORCH_CI_REQUIRED", refusal));
        }
        policy.ci.required = list
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect();
        if policy.ci.required.is_empty() {
            return Err(gate_var("ORCH_CI_REQUIRED", "names no check"));
        }
    }
    // A cap below the default lowers the default with it; an attempts value that is set is taken
    // as written and must fit.
    let attempts = number(
        vars.max_attempts,
        "ORCH_MAX_ATTEMPTS",
        DEFAULT_MAX_ATTEMPTS.min(cap),
        1,
    )?;
    if attempts > cap {
        return Err(gate_var(
            "ORCH_MAX_ATTEMPTS",
            format!("{attempts} is above ORCH_MAX_ATTEMPTS_CAP ({cap})"),
        ));
    }
    policy.max_attempts = attempts;
    if let Some(verifier) = vars.verifier {
        if let Some(refusal) = rules.refuse_verifier_setting(&at) {
            return Err(gate_var("ORCH_VERIFIER", refusal));
        }
        policy.verifier = Some(AgentId::new(verifier));
    }
    // The wait for the verifier is the deployment's to set; it bounds how long a thread waits
    // for a verdict, whatever the agent (ADR 0018).
    let verifier_timeout = number(
        vars.verifier_timeout_secs,
        "ORCH_VERIFIER_TIMEOUT_SECS",
        DEFAULT_VERIFIER_TIMEOUT_SECS,
        1,
    )?;
    policy.set_verifier_timeout_secs(verifier_timeout);

    // Each target's entry on top of the deployment, then every agent's verifier against the
    // configured agents. Both name the agent at fault.
    let refused = |e: orch_app::GateError| ConfigError::Gate {
        context: "AGENTS_FILE",
        reason: e.to_string(),
    };
    // The deployment's own policy first, so a fault in it is not blamed on an agent's entry.
    rules
        .check_policy(&policy, &at)
        .map_err(|e| gate_var("ORCH_GATE", e))?;
    for (id, layer) in targets {
        rules
            .for_target(&policy, id, Some(layer))
            .map_err(refused)?;
    }
    rules
        .validate(&policy, targets, &AgentDirectory::new(agents.to_vec()))
        .map_err(|e| match e {
            // A verifier the deployment names is the variable's fault, not a file's.
            e @ orch_app::GateError::UnknownVerifier {
                layer: Layer::Deployment,
                ..
            } => gate_var("ORCH_VERIFIER", e),
            e => refused(e),
        })?;
    Ok((policy, rules))
}

/// `ORCH_PUBLIC_URL`: an `http(s)` origin, no path, query or fragment; returned without a
/// trailing slash.
fn parse_public_url(raw: &str) -> Result<String, ConfigError> {
    let invalid = |reason: String| ConfigError::Invalid {
        var: "ORCH_PUBLIC_URL",
        reason,
    };
    let url = Url::parse(raw).map_err(|e| invalid(format!("{raw:?} is not a URL ({e})")))?;
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
        return Err(invalid(format!(
            "{raw:?} must be an http(s) URL with a host"
        )));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid(format!(
            "{raw:?} must be an origin such as https://chat.example.com, without a path"
        )));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// `MCP_ALLOWED_HOSTS`: at least one name.
fn parse_allowed_hosts(raw: Option<String>) -> Result<Vec<String>, ConfigError> {
    let raw = raw.ok_or(ConfigError::Missing("MCP_ALLOWED_HOSTS"))?;
    let hosts: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_owned)
        .collect();
    if hosts.is_empty() {
        return Err(ConfigError::Invalid {
            var: "MCP_ALLOWED_HOSTS",
            reason: "the list is empty; name the hosts clients use, such as orch.example.com"
                .to_owned(),
        });
    }
    // A `Host` header is an authority: a name or an address, with or without a port. A URL, a
    // wildcard or a path matches nothing, and would only look like a rule.
    if let Some(bad) = hosts.iter().find(|h| !is_authority(h)) {
        return Err(ConfigError::Invalid {
            var: "MCP_ALLOWED_HOSTS",
            reason: format!(
                "{bad:?} is not a host name or address, with or without a port (no scheme, \
                 path, wildcard or credentials): write orch.example.com or orch.example.com:8443"
            ),
        });
    }
    Ok(hosts)
}

/// Whether `host` is a `Host` header value: a name or an address, with or without a port.
/// (Kept here, beside `orch_surface_mcp::is_host_authority`, because this file also builds
/// without the `surface-mcp` feature.)
fn is_authority(host: &str) -> bool {
    if host.contains(['@', '*', '/', '?', '#', ' ']) {
        return false;
    }
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    if authority.as_str() != host || authority.host().is_empty() {
        return false;
    }
    match host[authority.host().len()..].strip_prefix(':') {
        None => host.len() == authority.host().len(),
        Some(port) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// `MCP_ALLOWED_ORIGINS`: `http(s)://host[:port]` entries, possibly none.
fn parse_allowed_origins(raw: Option<String>) -> Result<Vec<String>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let origins: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .map(str::to_owned)
        .collect();
    for origin in &origins {
        let ok = Url::parse(origin).is_ok_and(|u| {
            matches!(u.scheme(), "http" | "https")
                && u.has_host()
                && u.username().is_empty()
                && u.path() == "/"
                && u.query().is_none()
                && u.fragment().is_none()
        }) && !origin.ends_with('/');
        if !ok {
            return Err(ConfigError::Invalid {
                var: "MCP_ALLOWED_ORIGINS",
                reason: format!(
                    "{origin:?} is not an origin: write https://host or https://host:port, \
                     with no path"
                ),
            });
        }
    }
    Ok(origins)
}

/// `MCP_TOKENS_FILE`: the YAML list, with each token read from the variable its `tokenEnv` names.
/// A user may appear more than once (a rotation); a token may belong to one user only.
fn parse_mcp_tokens(
    path: Option<String>,
    env: &impl Fn(&str) -> Option<String>,
    read: &impl Fn(&Path) -> io::Result<String>,
) -> Result<Vec<(UserId, BTreeSet<orch_ports::Role>, SecretString)>, ConfigError> {
    let invalid = |reason: String| ConfigError::Invalid {
        var: "MCP_TOKENS_FILE",
        reason,
    };
    let path = PathBuf::from(path.ok_or(ConfigError::Missing("MCP_TOKENS_FILE"))?);
    let text = read(&path).map_err(|source| ConfigError::McpTokensFileRead {
        path: path.clone(),
        source,
    })?;
    let specs: Option<Vec<TokenSpec>> = serde_norway::from_str(&text).map_err(|e| {
        invalid(format!(
            "{} is not a list of {{user, tokenEnv, role?}}: {e}",
            path.display()
        ))
    })?;
    let specs = specs.unwrap_or_default();
    if specs.is_empty() {
        return Err(invalid(format!("{} lists no tokens", path.display())));
    }
    let mut tokens: Vec<(UserId, BTreeSet<orch_ports::Role>, SecretString)> =
        Vec::with_capacity(specs.len());
    for spec in specs {
        let user = spec.user.trim();
        if !user.contains('@') {
            return Err(invalid(format!(
                "user {user:?} must be the e-mail address the jobs belong to"
            )));
        }
        let var = spec.token_env.trim();
        if var.is_empty() {
            return Err(invalid(format!("the tokenEnv of {user:?} is empty")));
        }
        let token = env(var)
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| ConfigError::McpTokenEnvMissing {
                user: user.to_owned(),
                var: var.to_owned(),
            })?;
        if token.len() < MIN_MCP_TOKEN_BYTES {
            return Err(invalid(format!(
                "the token of {user:?} (in {var}) is shorter than {MIN_MCP_TOKEN_BYTES} bytes; \
                 generate one with `openssl rand -base64 32`"
            )));
        }
        let user = UserId::new(user);
        if let Some((other, ..)) = tokens
            .iter()
            .find(|(.., t)| t.expose_secret() == token.as_str())
        {
            return Err(invalid(format!(
                "the same token is configured for {other} and for {user}"
            )));
        }
        let roles = match spec.role.as_deref().map(str::trim) {
            None => BTreeSet::new(),
            Some("") => return Err(invalid(format!("the role of {user} is empty"))),
            Some(role) => BTreeSet::from([orch_ports::Role::new(role)]),
        };
        tokens.push((user, roles, SecretString::from(token)));
    }
    Ok(tokens)
}

/// The raw values of the thread-tools variables (blank already counted as unset).
#[cfg(feature = "surface-thread-tools")]
struct ThreadToolsVars {
    secret: Option<String>,
    previous: Option<String>,
    url: Option<String>,
    ttl_secs: Option<String>,
    allowed_hosts: Option<String>,
}

/// The thread-tools settings (`THREAD_TOOLS_*`): none when neither the secret nor the URL is set
/// and the surface is not named; both or neither otherwise. `mounted` is whether `ORCH_SURFACES`
/// names `thread-tools`, in which case they are required, in every role.
#[cfg(feature = "surface-thread-tools")]
fn thread_tools(
    vars: ThreadToolsVars,
    mounted: bool,
) -> Result<Option<ThreadToolsSettings>, ConfigError> {
    const SECRET: &str = "THREAD_TOOLS_SECRET";
    const PREVIOUS: &str = "THREAD_TOOLS_SECRET_PREVIOUS";
    const URL: &str = "THREAD_TOOLS_URL";
    let ttl = number(
        vars.ttl_secs,
        "THREAD_TOOLS_TOKEN_TTL_SECS",
        DEFAULT_TTL_SECS,
        MIN_TTL_SECS,
    )?;
    if ttl > MAX_TTL_SECS {
        return Err(ConfigError::Invalid {
            var: "THREAD_TOOLS_TOKEN_TTL_SECS",
            reason: format!("must be at most {MAX_TTL_SECS}"),
        });
    }
    // Read whatever else is set, so that a typo is refused here and not quietly ignored.
    let explicit_hosts = vars
        .allowed_hosts
        .map(|raw| parse_host_list("THREAD_TOOLS_ALLOWED_HOSTS", &raw))
        .transpose()?;
    let (secret, url) = match (vars.secret, vars.url) {
        (Some(secret), Some(url)) => (secret, url),
        (None, None) if mounted => {
            return Err(ConfigError::MissingForSurface {
                var: SECRET,
                surface: Surface::ThreadTools.name(),
            });
        }
        (None, None) => {
            // A previous key with no current one is a mistake to say, not to ignore.
            if vars.previous.is_some() {
                return Err(ConfigError::Invalid {
                    var: PREVIOUS,
                    reason: format!("{SECRET} is not set; the previous key only verifies"),
                });
            }
            return Ok(None);
        }
        (Some(_), None) => {
            return Err(ConfigError::Invalid {
                var: SECRET,
                reason: format!("{URL} is not set; set both or neither"),
            });
        }
        (None, Some(_)) => {
            return Err(ConfigError::Invalid {
                var: URL,
                reason: format!("{SECRET} is not set; set both or neither"),
            });
        }
    };
    let keys = ThreadToolsKeys::new(
        SecretString::from(secret),
        vars.previous.map(SecretString::from),
    )
    .map_err(|e| ConfigError::Invalid {
        var: match e {
            orch_thread_token::KeyError::TooShort { which: "previous" }
            | orch_thread_token::KeyError::Same => PREVIOUS,
            _ => SECRET,
        },
        reason: e.to_string(),
    })?;
    let issuer = ThreadToolsIssuer::new(keys, &url, Duration::from_secs(ttl)).map_err(|e| {
        ConfigError::Invalid {
            var: URL,
            reason: e.to_string(),
        }
    })?;
    let allowed_hosts = explicit_hosts.unwrap_or_else(|| vec![issuer.host().to_owned()]);
    Ok(Some(ThreadToolsSettings {
        issuer: Arc::new(issuer),
        allowed_hosts,
    }))
}

/// A comma-separated list of `Host` values: at least one, each a name or an address with or
/// without a port.
#[cfg(feature = "surface-thread-tools")]
fn parse_host_list(var: &'static str, raw: &str) -> Result<Vec<String>, ConfigError> {
    let hosts: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_owned)
        .collect();
    if hosts.is_empty() {
        return Err(ConfigError::Invalid {
            var,
            reason: "the list is empty; name the hosts agents use, such as orchestrator:8080"
                .to_owned(),
        });
    }
    if let Some(bad) = hosts.iter().find(|h| !is_authority(h)) {
        return Err(ConfigError::Invalid {
            var,
            reason: format!(
                "{bad:?} is not a host name or address, with or without a port (no scheme, \
                 path, wildcard or credentials): write orchestrator:8080 or orch.example.com"
            ),
        });
    }
    Ok(hosts)
}

/// The secrets of a webhook route as they arrive: one string of one or two comma-separated
/// secrets (the variable), or the secrets already separated (the configuration file, where a
/// secret may contain a comma).
#[cfg(feature = "surface-webhook")]
enum WebhookSecrets {
    Joined(String),
    Separate(Vec<String>),
}

#[cfg(feature = "surface-webhook")]
impl WebhookSecrets {
    fn parse(self) -> Result<Secrets, orch_surface_webhook::SecretsError> {
        match self {
            WebhookSecrets::Joined(raw) => Secrets::parse(&raw),
            WebhookSecrets::Separate(all) => Secrets::new(all.into_iter().map(SecretString::from)),
        }
    }
}

/// What the typed file path hands to [`Config::load_with`] beside [`Args`]: the values that
/// cannot go through a string without being read again (a secret that has a comma in it).
#[derive(Default)]
struct Resolved {
    /// `artifacts`, read as the file said it (it has no variable).
    artifacts: Option<ArtifactSettings>,
    /// `auth.mode` and `auth.jwt`: keys that have no variable, so no string to go through.
    auth: AuthSettings,
    /// `models.endpoints` and `tasks`, read: they replace what the legacy variables make.
    models: Option<ModelsSettings>,
    /// `ui`, for `GET /api/config`.
    public: Option<PublicConfig>,
    /// `toolServers`, the public part of each server.
    tool_servers: Vec<ToolServerInfo>,
    /// `toolServers`, the URL and the credentials of each, for the relay.
    tool_endpoints: Vec<ToolServerEndpoint>,
    /// `webhooks.generic.secrets`, separated.
    #[cfg(feature = "surface-webhook")]
    webhook_generic: Option<Vec<String>>,
    /// `webhooks.github.secrets`, separated.
    #[cfg(feature = "surface-webhook")]
    webhook_github: Option<Vec<String>>,
}

/// The generic webhook route's settings (`WEBHOOK_GENERIC_*`). The secrets are required exactly
/// when the route is to be mounted (`mounted`: `webhook-generic` is in `ORCH_SURFACES` and this
/// role serves HTTP routes), so a worker that shares the environment of a control plane does not
/// need the secret; a value that is set is checked either way.
#[cfg(feature = "surface-webhook")]
fn webhook_generic(
    secrets: Option<WebhookSecrets>,
    max_skew_secs: Option<String>,
    mounted: bool,
) -> Result<Option<GenericConfig>, ConfigError> {
    const VAR: &str = "WEBHOOK_GENERIC_SECRETS";
    let max_skew = number(
        max_skew_secs,
        "WEBHOOK_GENERIC_MAX_SKEW_SECS",
        DEFAULT_WEBHOOK_MAX_SKEW_SECS,
        1,
    )?;
    let secrets = match secrets {
        Some(raw) => Some(raw.parse().map_err(|e| ConfigError::Invalid {
            var: VAR,
            reason: e.to_string(),
        })?),
        None if mounted => {
            return Err(ConfigError::MissingForSurface {
                var: VAR,
                surface: Surface::WebhookGeneric.name(),
            });
        }
        None => None,
    };
    Ok(secrets.map(|secrets| GenericConfig {
        max_skew: Duration::from_secs(max_skew),
        ..GenericConfig::new(secrets)
    }))
}

/// The GitHub webhook route's settings (`WEBHOOK_GITHUB_SECRETS`), required exactly when the route
/// is to be mounted, like [`webhook_generic`].
#[cfg(feature = "surface-webhook")]
fn webhook_github(
    secrets: Option<WebhookSecrets>,
    max_age_secs: Option<String>,
    mounted: bool,
) -> Result<Option<GithubConfig>, ConfigError> {
    const VAR: &str = "WEBHOOK_GITHUB_SECRETS";
    let max_age = number(
        max_age_secs,
        "WEBHOOK_GITHUB_MAX_AGE_SECS",
        orch_surface_webhook::github::DEFAULT_MAX_AGE_SECS,
        1,
    )?;
    match secrets {
        Some(raw) => raw
            .parse()
            .map(|secrets| {
                Some(GithubConfig {
                    max_age: Duration::from_secs(max_age),
                    ..GithubConfig::new(secrets)
                })
            })
            .map_err(|e| ConfigError::Invalid {
                var: VAR,
                reason: e.to_string(),
            }),
        None if mounted => Err(ConfigError::MissingForSurface {
            var: VAR,
            surface: Surface::WebhookGithub.name(),
        }),
        None => Ok(None),
    }
}

/// Parses an optional numeric variable with a default and a lower bound.
fn number<T>(raw: Option<String>, var: &'static str, default: T, min: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr + PartialOrd + fmt::Display + Copy,
    T::Err: fmt::Display,
{
    let Some(raw) = raw else { return Ok(default) };
    let value = raw.parse::<T>().map_err(|e| ConfigError::Invalid {
        var,
        reason: format!("{raw:?} is not a number ({e})"),
    })?;
    if value < min {
        return Err(ConfigError::Invalid {
            var,
            reason: format!("must be at least {min}"),
        });
    }
    Ok(value)
}

/// Parses an optional boolean variable with a default: `true`/`false`, `1`/`0`, `yes`/`no`,
/// `on`/`off`, in any case. Anything else is refused, never read as a default.
fn flag(raw: Option<String>, var: &'static str, default: bool) -> Result<bool, ConfigError> {
    let Some(raw) = raw else { return Ok(default) };
    match raw.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::Invalid {
            var,
            reason: format!("{raw:?} is not true or false"),
        }),
    }
}

fn invalid(id: &str, reason: impl Into<String>) -> ConfigError {
    ConfigError::InvalidAgent {
        id: id.to_owned(),
        reason: reason.into(),
    }
}

/// Parses the YAML list and resolves each `tokenEnv` through `env`.
#[cfg(test)]
fn parse_agents(
    yaml: &str,
    path: &Path,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<AgentEntry>, ConfigError> {
    parse_agents_with(yaml, path, env, LocalAgentKind::compiled_in)
}

/// [`parse_agents`] with the build's set of local agent kinds as a parameter, so the tests can
/// exercise a build that has them.
#[cfg(test)]
fn parse_agents_with(
    yaml: &str,
    path: &Path,
    env: impl Fn(&str) -> Option<String>,
    local_compiled_in: impl Fn(LocalAgentKind) -> bool,
) -> Result<Vec<AgentEntry>, ConfigError> {
    parse_agents_full(yaml, path, env, local_compiled_in, false).map(|(entries, _)| entries)
}

/// The entries of the YAML list and the `gate` key of each that has one.
///
/// `allow_empty`: a file that lists no agent is fine (a registry lists them).
fn parse_agents_full(
    yaml: &str,
    path: &Path,
    env: impl Fn(&str) -> Option<String>,
    local_compiled_in: impl Fn(LocalAgentKind) -> bool,
    allow_empty: bool,
) -> Result<(Vec<AgentEntry>, BTreeMap<AgentId, GateLayer>), ConfigError> {
    let specs: Option<Vec<AgentSpec>> =
        serde_norway::from_str(yaml).map_err(|e| ConfigError::AgentsFileParse {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
    let specs = specs.unwrap_or_default();
    if specs.is_empty() && !allow_empty {
        return Err(ConfigError::NoAgents {
            path: path.to_owned(),
        });
    }

    let mut entries: Vec<AgentEntry> = Vec::with_capacity(specs.len());
    let mut gates: BTreeMap<AgentId, GateLayer> = BTreeMap::new();
    for spec in specs {
        if !orch_core::is_valid_agent_id(&spec.id) {
            return Err(invalid(
                &spec.id,
                "id must match ^[a-z0-9][a-z0-9-]{0,62}$ (lower-case letters, digits, dashes)",
            ));
        }
        if entries.iter().any(|e| e.endpoint.id.as_str() == spec.id) {
            return Err(ConfigError::DuplicateAgent(spec.id));
        }
        if spec.name.trim().is_empty() {
            return Err(invalid(&spec.id, "name must not be empty"));
        }
        // A new `TransportKind` makes this `match` fail to compile: each transport validates
        // the keys that belong to it, and refuses the keys that belong to another.
        let endpoint = match spec.transport {
            TransportKind::A2a => a2a_endpoint(&spec, &env)?,
            TransportKind::Local => local_endpoint(&spec, &local_compiled_in)?,
        };
        if let Some(layer) = spec.gate {
            gates.insert(AgentId::new(spec.id.clone()), layer);
        }
        entries.push(AgentEntry {
            endpoint,
            name: spec.name.trim().to_owned(),
        });
    }
    Ok((entries, gates))
}

/// `transport: a2a`: a card URL and an optional token variable.
fn a2a_endpoint(
    spec: &AgentSpec,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<AgentEndpoint, ConfigError> {
    if spec.agent.is_some() {
        return Err(invalid(
            &spec.id,
            "agent applies to transport local only (an a2a agent is named by its cardUrl)",
        ));
    }
    let Some(card_url) = spec.card_url.as_deref() else {
        return Err(invalid(&spec.id, "cardUrl is required for transport a2a"));
    };
    match Url::parse(card_url) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.has_host() => {}
        Ok(_) => {
            return Err(invalid(
                &spec.id,
                "cardUrl must be an http(s) URL with a host",
            ));
        }
        Err(e) => return Err(invalid(&spec.id, format!("cardUrl is not a URL ({e})"))),
    }
    let bearer = match spec.token_env.as_deref().map(str::trim) {
        None => None,
        Some("") => {
            return Err(invalid(&spec.id, "tokenEnv must not be empty when present"));
        }
        Some(var) => match env(var) {
            Some(token) => Some(token),
            None => {
                return Err(ConfigError::TokenEnvMissing {
                    agent: spec.id.clone(),
                    var: var.to_owned(),
                });
            }
        },
    };
    Ok(AgentEndpoint::a2a(
        AgentId::new(spec.id.clone()),
        card_url,
        bearer,
    ))
}

/// `transport: local`: the kind of agent, and nothing that belongs to a remote one. The keys of
/// an A2A agent are refused, not ignored: a `cardUrl` next to `transport: local` is a mistake
/// (probably a lost `transport: a2a`), and the operator should hear about it at startup.
fn local_endpoint(
    spec: &AgentSpec,
    compiled_in: &impl Fn(LocalAgentKind) -> bool,
) -> Result<AgentEndpoint, ConfigError> {
    if spec.card_url.is_some() || spec.token_env.is_some() {
        return Err(invalid(
            &spec.id,
            "cardUrl and tokenEnv apply to transport a2a only (a local agent has no card and no token)",
        ));
    }
    let Some(name) = spec.agent.as_deref().map(str::trim) else {
        return Err(invalid(
            &spec.id,
            format!(
                "agent is required for transport local (one of: {})",
                LocalAgentKind::known()
            ),
        ));
    };
    let Some(kind) = LocalAgentKind::parse(name) else {
        return Err(invalid(
            &spec.id,
            format!(
                "unknown local agent {name:?} (one of: {})",
                LocalAgentKind::known()
            ),
        ));
    };
    if !compiled_in(kind) {
        return Err(ConfigError::LocalAgentsNotCompiled {
            agent: spec.id.clone(),
            feature: kind.feature(),
        });
    }
    Ok(AgentEndpoint::local(
        AgentId::new(spec.id.clone()),
        kind.name(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use orch_ports::AgentTransport;

    use super::*;

    const AGENTS: &str = "\
- id: coder
  name: Coder
  cardUrl: https://coder.example.com/.well-known/agent-card.json
  tokenEnv: CODER_A2A_TOKEN
- id: plain
  name: Plain
  cardUrl: http://plain.internal:9000/.well-known/agent-card.json
";

    pub(super) fn env_of<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Option<String> + 'a {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    /// The `Args` the process environment `pairs` would produce, built by hand so no test
    /// depends on (or changes) the real environment. Variables that are not settings of the
    /// service (the `tokenEnv` ones) stay in `pairs` and are read through `env_of`.
    pub(super) fn args_of(pairs: &[(&str, &str)]) -> Args {
        let mut args = Args::default();
        for (name, value) in pairs {
            let slot = match *name {
                "ORCH_CONFIG_FILE" => &mut args.config,
                "DATABASE_URL" => &mut args.database_url,
                "AGENTS_FILE" => &mut args.agents_file,
                "AGENT_REGISTRY_URL" => &mut args.registry_url,
                "AGENT_REGISTRY_TOKEN" => &mut args.registry_token,
                "AGENT_REGISTRY_AGENT_TOKEN" => &mut args.registry_agent_token,
                "AGENT_REGISTRY_TIMEOUT_SECS" => &mut args.registry_timeout_secs,
                "AGENT_REGISTRY_MAX_AGE_SECS" => &mut args.registry_max_age_secs,
                "LISTEN_ADDR" => &mut args.listen_addr,
                "ORCH_ROLE" => &mut args.role,
                "ORCH_SURFACES" => &mut args.surfaces,
                "ORCH_GATE" => &mut args.gate,
                "ORCH_MAX_ATTEMPTS" => &mut args.max_attempts,
                "ORCH_MAX_ATTEMPTS_CAP" => &mut args.max_attempts_cap,
                "ORCH_VERIFIER" => &mut args.verifier,
                "MCP_TOKENS_FILE" => &mut args.mcp_tokens_file,
                "MCP_ALLOWED_HOSTS" => &mut args.mcp_allowed_hosts,
                "ORCH_PUBLIC_URL" => &mut args.public_url,
                "MCP_WAIT_MAX_SECS" => &mut args.mcp_wait_max_secs,
                "MCP_WAIT_MAX_CONCURRENT" => &mut args.mcp_wait_max_concurrent,
                "MCP_WAIT_MAX_PER_USER" => &mut args.mcp_wait_max_per_user,
                "MCP_ALLOWED_ORIGINS" => &mut args.mcp_allowed_origins,
                "THREAD_TOOLS_SECRET" => &mut args.thread_tools_secret,
                "THREAD_TOOLS_SECRET_PREVIOUS" => &mut args.thread_tools_secret_previous,
                "THREAD_TOOLS_URL" => &mut args.thread_tools_url,
                "THREAD_TOOLS_TOKEN_TTL_SECS" => &mut args.thread_tools_token_ttl_secs,
                "THREAD_TOOLS_ALLOWED_HOSTS" => &mut args.thread_tools_allowed_hosts,
                "ORCH_VERIFIER_TIMEOUT_SECS" => &mut args.verifier_timeout_secs,
                "ORCH_VERIFIER_WATCH_SECS" => &mut args.verifier_watch_secs,
                "ORCH_STEPS_RECORD_IO" => &mut args.steps_record_io,
                "ORCH_TITLE_MODEL" => &mut args.title_model,
                "ORCH_MODEL_BASE_URL" => &mut args.model_base_url,
                "ORCH_MODEL_API_KEY" => &mut args.model_api_key,
                "ORCH_MODEL_TIMEOUT_SECS" => &mut args.model_timeout_secs,
                "ORCH_CI_TIMEOUT_SECS" => &mut args.ci_timeout_secs,
                "ORCH_CI_REQUIRED" => &mut args.ci_required,
                "WEBHOOK_GENERIC_SECRETS" => &mut args.webhook_generic_secrets,
                "WEBHOOK_GITHUB_SECRETS" => &mut args.webhook_github_secrets,
                "WEBHOOK_GITHUB_MAX_AGE_SECS" => &mut args.webhook_github_max_age_secs,
                "WEBHOOK_GENERIC_MAX_SKEW_SECS" => &mut args.webhook_generic_max_skew_secs,
                "AUTH_DEV_USER" => &mut args.auth_dev_user,
                "DATABASE_MAX_CONNECTIONS" => &mut args.database_max_connections,
                "DISPATCHER_CONCURRENCY" => &mut args.dispatcher_concurrency,
                #[cfg(feature = "agent-local")]
                "AGENT_LOCAL_CONCURRENCY" => &mut args.agent_local_concurrency,
                "OUTBOX_LEASE_SECS" => &mut args.outbox_lease_secs,
                "INBOX_LEASE_SECS" => &mut args.inbox_lease_secs,
                "INBOX_POLL_SECS" => &mut args.inbox_poll_secs,
                "INBOX_PARKED_TTL_SECS" => &mut args.inbox_parked_ttl_secs,
                "INBOX_MAX_ATTEMPTS" => &mut args.inbox_max_attempts,
                "SHUTDOWN_GRACE_SECS" => &mut args.shutdown_grace_secs,
                "ORCH_INSTANCE_ID" => &mut args.instance_id,
                "LOG_FORMAT" => &mut args.log_format,
                "HOSTNAME" => &mut args.hostname,
                _ => continue,
            };
            *slot = Some((*value).to_owned());
        }
        args
    }

    fn load(pairs: &[(&str, &str)], agents_yaml: &str) -> Result<Config, ConfigError> {
        let yaml = agents_yaml.to_owned();
        Config::load(args_of(pairs), env_of(pairs), move |_| Ok(yaml.clone()))
    }

    fn base<'a>() -> Vec<(&'a str, &'a str)> {
        vec![
            ("DATABASE_URL", "postgres://u:secret@db/orch"),
            ("AGENTS_FILE", "/etc/orch/agents.yaml"),
            ("CODER_A2A_TOKEN", "tok-123"),
        ]
    }

    /// The card URL and bearer of an A2A endpoint.
    fn a2a(endpoint: &AgentEndpoint) -> (&str, Option<&str>) {
        let AgentTransport::A2a { card_url, bearer } = &endpoint.transport else {
            panic!("expected an A2A endpoint, got {endpoint:?}");
        };
        (card_url, bearer.as_deref())
    }

    fn agents_err(yaml: &str, env: &[(&str, &str)]) -> ConfigError {
        match parse_agents(yaml, Path::new("agents.yaml"), env_of(env)) {
            Ok(_) => panic!("expected an error"),
            Err(e) => e,
        }
    }

    #[test]
    fn defaults_apply_when_only_the_required_variables_are_set() {
        let cfg = load(&base(), AGENTS).unwrap();
        assert_eq!(cfg.listen_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(cfg.database_max_connections, 10);
        assert_eq!(cfg.dispatcher_concurrency, 32);
        assert_eq!(cfg.outbox_lease, Duration::from_secs(30));
        assert_eq!(cfg.verifier_watch, Duration::from_secs(5));
        assert_eq!(cfg.inbox.lease, Duration::from_secs(30));
        assert_eq!(cfg.inbox.poll_interval, Duration::from_secs(2));
        assert_eq!(cfg.inbox.parked_ttl, Duration::from_secs(86_400));
        assert_eq!(cfg.inbox.max_attempts, 10);
        assert_eq!(cfg.shutdown_grace, Duration::from_secs(15));
        assert!(cfg.auth_dev_user.is_none());
        assert!(cfg.instance_id.starts_with("orchestrator-"));
    }

    #[test]
    fn a_steps_input_and_output_are_recorded_unless_the_switch_says_otherwise() {
        let cfg = load(&base(), AGENTS).unwrap();
        assert!(cfg.steps_record_io, "the owner's default of 2026-10-02: on");
        assert!(cfg.app_config().record_step_io);
        for off in ["false", "0", "no", "off", "FALSE", "Off"] {
            let mut env = base();
            env.push(("ORCH_STEPS_RECORD_IO", off));
            let cfg = load(&env, AGENTS).unwrap();
            assert!(!cfg.steps_record_io, "{off}");
            assert!(!cfg.app_config().record_step_io, "{off}");
        }
        for on in ["true", "1", "yes", "on"] {
            let mut env = base();
            env.push(("ORCH_STEPS_RECORD_IO", on));
            assert!(load(&env, AGENTS).unwrap().steps_record_io, "{on}");
        }
        // a value that is neither is refused, not read as the default
        let mut env = base();
        env.push(("ORCH_STEPS_RECORD_IO", "maybe"));
        assert!(matches!(
            load(&env, AGENTS),
            Err(ConfigError::Invalid {
                var: "ORCH_STEPS_RECORD_IO",
                ..
            })
        ));
    }

    #[test]
    fn titles_are_off_unless_a_model_is_named() {
        let cfg = load(&base(), AGENTS).unwrap();
        let app = cfg.app_config();
        assert!(app.tasks.is_empty());
        assert!(cfg.models.tasks.is_empty() && cfg.models.endpoints.is_empty());
        assert!(app.public.ui.show_descriptions);
        // the endpoint variables alone turn nothing on
        let mut env = base();
        env.extend([
            ("ORCH_MODEL_BASE_URL", "https://api.example.com/v1"),
            ("ORCH_MODEL_API_KEY", "sk-secret"),
        ]);
        assert!(load(&env, AGENTS).unwrap().models.tasks.is_empty());
    }

    #[test]
    fn a_title_model_needs_its_endpoint_and_reaches_the_application() {
        let mut env = base();
        env.push(("ORCH_TITLE_MODEL", "gpt-4o-mini"));
        assert!(matches!(
            load(&env, AGENTS),
            Err(ConfigError::Missing("ORCH_MODEL_BASE_URL"))
        ));

        env.push(("ORCH_MODEL_BASE_URL", "https://api.example.com/v1/"));
        env.push(("ORCH_MODEL_API_KEY", "sk-secret"));
        let cfg = load(&env, AGENTS).unwrap();
        // the endpoint `default` and the title task on it
        let endpoint = &cfg.models.endpoints["default"];
        assert_eq!(
            endpoint.base_url, "https://api.example.com/v1",
            "no trailing slash"
        );
        assert_eq!(endpoint.timeout, Duration::from_secs(20));
        assert_eq!(cfg.models.endpoints.len(), 1);
        let app = cfg.app_config();
        assert_eq!(app.tasks.len(), 1, "descriptions are off");
        let title = &app.tasks[&TaskKind::Title];
        assert_eq!(
            (title.endpoint.as_str(), title.model.as_str()),
            ("default", "gpt-4o-mini")
        );
        assert_eq!(title.timeout, Duration::from_secs(20));
        assert_eq!(title.max_tokens, 32);
        assert_eq!(title.guidance, None);

        // the key is in no debug text
        assert!(!format!("{cfg:?}").contains("sk-secret"));
        assert!(format!("{cfg:?}").contains("<redacted>"));
    }

    #[test]
    fn the_model_variables_are_validated() {
        let with = |extra: &[(&'static str, &'static str)]| {
            let mut env = base();
            env.extend_from_slice(extra);
            load(&env, AGENTS)
        };
        for bad in [
            "ftp://example.com",
            "localhost:8080/v1",
            "https://",
            "example.com",
        ] {
            let err = with(&[("ORCH_TITLE_MODEL", "m"), ("ORCH_MODEL_BASE_URL", bad)]).unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConfigError::Invalid {
                        var: "ORCH_MODEL_BASE_URL",
                        ..
                    }
                ),
                "{bad}: {err:?}"
            );
        }
        for bad in ["0", "soon"] {
            // refused whether or not titles are on
            let err = with(&[("ORCH_MODEL_TIMEOUT_SECS", bad)]).unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConfigError::Invalid {
                        var: "ORCH_MODEL_TIMEOUT_SECS",
                        ..
                    }
                ),
                "{bad}: {err:?}"
            );
        }
        let cfg = with(&[
            ("ORCH_TITLE_MODEL", "m"),
            ("ORCH_MODEL_BASE_URL", "http://mock-model:8080/v1"),
            ("ORCH_MODEL_TIMEOUT_SECS", "7"),
        ])
        .unwrap();
        assert_eq!(
            cfg.app_config().tasks[&TaskKind::Title].timeout,
            Duration::from_secs(7)
        );
        assert!(cfg.models.endpoints["default"].api_key.is_none());
    }

    /// An environment with a registry URL and no `AGENTS_FILE`.
    fn registry_only<'a>() -> Vec<(&'a str, &'a str)> {
        let mut env = base();
        env.retain(|(k, _)| *k != "AGENTS_FILE");
        env.push((
            "AGENT_REGISTRY_URL",
            "https://platform.example.com/registry/v1/agents",
        ));
        env
    }

    #[cfg(feature = "registry-platform")]
    #[test]
    fn there_is_no_registry_unless_its_url_is_set() {
        let cfg = load(&base(), AGENTS).unwrap();
        assert!(cfg.registry.is_none());
        // the other variables alone turn nothing on
        let mut env = base();
        env.extend([
            ("AGENT_REGISTRY_TOKEN", "reg-secret"),
            ("AGENT_REGISTRY_AGENT_TOKEN", "agent-secret"),
        ]);
        assert!(load(&env, AGENTS).unwrap().registry.is_none());
    }

    #[cfg(feature = "registry-platform")]
    #[test]
    fn a_registry_is_read_with_its_defaults_and_its_secrets_stay_out_of_every_debug() {
        let mut env = base();
        env.extend([
            (
                "AGENT_REGISTRY_URL",
                "http://platform.agents.svc:8080/registry/v1/agents?tenant=secret-tenant",
            ),
            ("AGENT_REGISTRY_TOKEN", "reg-secret"),
            ("AGENT_REGISTRY_AGENT_TOKEN", "agent-secret"),
        ]);
        let cfg = load(&env, AGENTS).unwrap();
        let registry = cfg.registry.as_ref().unwrap();
        assert_eq!(
            registry.url,
            "http://platform.agents.svc:8080/registry/v1/agents?tenant=secret-tenant"
        );
        assert_eq!(registry.timeout, Duration::from_secs(3));
        assert_eq!(registry.max_age, Duration::from_secs(60));
        assert_eq!(cfg.agents.len(), 2, "the file's agents are still there");
        for text in [format!("{cfg:?}"), format!("{registry:?}")] {
            assert!(!text.contains("reg-secret"), "{text}");
            assert!(!text.contains("agent-secret"), "{text}");
            assert!(
                !text.contains("secret-tenant"),
                "no query in a log line: {text}"
            );
            assert!(text.contains("<redacted>"), "{text}");
            assert!(
                text.contains("platform.agents.svc:8080/registry/v1/agents"),
                "{text}"
            );
        }

        env.extend([
            ("AGENT_REGISTRY_TIMEOUT_SECS", "10"),
            ("AGENT_REGISTRY_MAX_AGE_SECS", "3600"),
        ]);
        let cfg = load(&env, AGENTS).unwrap();
        let registry = cfg.registry.unwrap();
        assert_eq!(registry.timeout, Duration::from_secs(10));
        assert_eq!(registry.max_age, Duration::from_secs(3600));
    }

    #[cfg(feature = "registry-platform")]
    #[test]
    fn the_registry_url_must_be_an_absolute_http_url_without_credentials() {
        for bad in [
            "platform.example.com/registry",
            "/registry/v1/agents",
            "ftp://platform.example.com/registry",
            "file:///etc/passwd",
            "https://",
            "https://user:pw@platform.example.com/registry/v1/agents",
            "https://user@platform.example.com/registry/v1/agents",
        ] {
            let mut env = base();
            env.push(("AGENT_REGISTRY_URL", bad));
            let err = load(&env, AGENTS).unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConfigError::Invalid {
                        var: "AGENT_REGISTRY_URL",
                        ..
                    }
                ),
                "{bad}: {err:?}"
            );
            assert!(!err.to_string().contains("pw"), "{err}");
        }
    }

    #[test]
    fn the_registry_numbers_are_validated_whether_or_not_a_registry_is_set() {
        for (var, bad) in [
            ("AGENT_REGISTRY_TIMEOUT_SECS", "0"),
            ("AGENT_REGISTRY_TIMEOUT_SECS", "61"),
            ("AGENT_REGISTRY_TIMEOUT_SECS", "soon"),
            ("AGENT_REGISTRY_MAX_AGE_SECS", "0"),
            ("AGENT_REGISTRY_MAX_AGE_SECS", "3601"),
            ("AGENT_REGISTRY_MAX_AGE_SECS", "-1"),
        ] {
            let mut env = base();
            env.push((var, bad));
            let err = load(&env, AGENTS).unwrap_err();
            match &err {
                ConfigError::Invalid { var: got, .. } => assert_eq!(*got, var, "{bad}"),
                other => panic!("{var}={bad}: {other:?}"),
            }
        }
    }

    #[cfg(feature = "registry-platform")]
    #[test]
    fn agents_file_is_optional_when_a_registry_is_set() {
        // unset
        let cfg = load(&registry_only(), AGENTS).unwrap();
        assert!(cfg.agents.is_empty());
        assert!(cfg.target_gates.is_empty());
        assert!(cfg.registry.is_some());
        // set, and listing nothing
        let mut env = base();
        env.push((
            "AGENT_REGISTRY_URL",
            "https://platform.example.com/registry/v1/agents",
        ));
        for empty in ["[]", "", "# nothing yet\n"] {
            let cfg = load(&env, empty).unwrap();
            assert!(cfg.agents.is_empty(), "{empty:?}");
        }
        // set, and listing agents: they stay, and come first
        let cfg = load(&env, AGENTS).unwrap();
        assert_eq!(cfg.agents.len(), 2);
        // a file that is set is still read and still validated
        let err = load(
            &env,
            "- id: Bad_Id\n  name: X\n  cardUrl: https://x.example.com/c\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::InvalidAgent { .. }), "{err:?}");
    }

    #[test]
    fn without_a_registry_the_agent_file_is_still_required_and_must_list_agents() {
        let mut env = base();
        env.retain(|(k, _)| *k != "AGENTS_FILE");
        assert!(matches!(
            load(&env, AGENTS),
            Err(ConfigError::Missing("AGENTS_FILE"))
        ));
        assert!(matches!(
            load(&base(), "[]"),
            Err(ConfigError::NoAgents { .. })
        ));
    }

    #[cfg(not(feature = "registry-platform"))]
    #[test]
    fn a_registry_url_in_a_build_without_the_registry_is_refused_not_ignored() {
        let err = load(&registry_only(), AGENTS).unwrap_err();
        assert!(matches!(err, ConfigError::RegistryNotCompiled), "{err:?}");
        assert!(err.to_string().contains("registry-platform"), "{err}");
        let mut env = base();
        env.push((
            "AGENT_REGISTRY_URL",
            "https://platform.example.com/registry/v1/agents",
        ));
        assert!(matches!(
            load(&env, AGENTS),
            Err(ConfigError::RegistryNotCompiled)
        ));
        // and without a URL nothing changes
        assert_eq!(load(&base(), AGENTS).unwrap().agents.len(), 2);
    }

    #[test]
    fn the_agent_list_is_parsed_and_tokens_are_read_from_their_variables() {
        let cfg = load(&base(), AGENTS).unwrap();
        assert_eq!(cfg.agents.len(), 2);
        let coder = &cfg.agents[0];
        assert_eq!(coder.endpoint.id.as_str(), "coder");
        assert_eq!(coder.name, "Coder");
        assert_eq!(
            a2a(&coder.endpoint),
            (
                "https://coder.example.com/.well-known/agent-card.json",
                Some("tok-123")
            )
        );
        assert_eq!(a2a(&cfg.agents[1].endpoint).1, None);
        assert!(!format!("{:?}", coder.endpoint).contains("tok-123"));
    }

    #[test]
    fn a_token_is_trimmed_because_secret_files_end_with_a_newline() {
        let mut env = base();
        env.retain(|(k, _)| *k != "CODER_A2A_TOKEN");
        env.push(("CODER_A2A_TOKEN", " tok-123\n"));
        // The load helper trims through the same `get` used for every variable.
        let cfg = load(&env, AGENTS).unwrap();
        assert_eq!(a2a(&cfg.agents[0].endpoint).1, Some("tok-123"));
    }

    #[test]
    fn a_missing_token_variable_fails_startup_and_names_it() {
        let mut env = base();
        env.retain(|(k, _)| *k != "CODER_A2A_TOKEN");
        let err = load(&env, AGENTS).unwrap_err();
        assert!(matches!(&err, ConfigError::TokenEnvMissing { agent, var }
            if agent == "coder" && var == "CODER_A2A_TOKEN"));
        assert!(err.to_string().contains("CODER_A2A_TOKEN"));
    }

    #[test]
    fn an_empty_token_variable_fails_startup() {
        let mut env = base();
        env.retain(|(k, _)| *k != "CODER_A2A_TOKEN");
        env.push(("CODER_A2A_TOKEN", "  "));
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::TokenEnvMissing { .. }
        ));
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let yaml = "\
- {id: a, name: A, cardUrl: 'https://a.example.com/card'}
- {id: a, name: B, cardUrl: 'https://b.example.com/card'}
";
        assert!(matches!(
            agents_err(yaml, &[]),
            ConfigError::DuplicateAgent(id) if id == "a"
        ));
    }

    #[test]
    fn ids_must_be_safe_slugs() {
        for bad in ["", "Coder", "-x", "has space", "a/b", "é", &"a".repeat(64)] {
            let yaml = format!(
                "- id: {}\n  name: X\n  cardUrl: https://x.example.com/card\n",
                serde_norway::to_string(bad).unwrap().trim()
            );
            assert!(
                matches!(agents_err(&yaml, &[]), ConfigError::InvalidAgent { .. }),
                "{bad:?} must be refused"
            );
        }
        assert!(orch_core::is_valid_agent_id("a"));
        assert!(orch_core::is_valid_agent_id("coder-2"));
        assert!(orch_core::is_valid_agent_id(&"a".repeat(63)));
    }

    #[test]
    fn card_urls_must_be_http_urls() {
        for bad in [
            "not a url",
            "ftp://x.example.com/card",
            "file:///etc/passwd",
            "https://",
        ] {
            let yaml = format!("- id: a\n  name: A\n  cardUrl: '{bad}'\n");
            assert!(
                matches!(agents_err(&yaml, &[]), ConfigError::InvalidAgent { .. }),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn unknown_fields_and_wrong_shapes_are_parse_errors() {
        let typo = "- id: a\n  name: A\n  cardUrl: https://a.example.com/card\n  tokenenv: X\n";
        assert!(matches!(
            agents_err(typo, &[]),
            ConfigError::AgentsFileParse { .. }
        ));
        assert!(matches!(
            agents_err("id: a\nname: A\n", &[]),
            ConfigError::AgentsFileParse { .. }
        ));
        assert!(matches!(
            agents_err("- id: a\n", &[]),
            ConfigError::AgentsFileParse { .. }
        ));
    }

    #[test]
    fn the_transport_defaults_to_a2a_and_unknown_transports_are_refused() {
        let plain = "- id: a\n  name: A\n  cardUrl: https://a.example.com/card.json\n";
        let explicit = format!("{plain}  transport: a2a\n");
        let want = |yaml: &str| {
            let entries = parse_agents(yaml, Path::new("agents.yaml"), env_of(&[])).unwrap();
            entries[0].endpoint.clone()
        };
        assert_eq!(want(plain), want(&explicit));
        assert_eq!(
            want(plain),
            AgentEndpoint::a2a(AgentId::new("a"), "https://a.example.com/card.json", None)
        );
        let ConfigError::AgentsFileParse { message, .. } =
            agents_err(&format!("{plain}  transport: grpc\n"), &[])
        else {
            panic!("expected a parse error");
        };
        assert!(
            message.contains("local") && message.contains("a2a"),
            "{message}"
        );
    }

    #[test]
    fn a_card_url_is_required_for_a2a_and_agent_is_refused() {
        let ConfigError::InvalidAgent { id, reason } = agents_err("- id: a\n  name: A\n", &[])
        else {
            panic!("expected InvalidAgent");
        };
        assert_eq!(id, "a");
        assert!(reason.contains("cardUrl is required"), "{reason}");
        let mixed = "- id: a\n  name: A\n  cardUrl: https://a.example.com/card\n  agent: echo\n";
        let ConfigError::InvalidAgent { reason, .. } = agents_err(mixed, &[]) else {
            panic!("expected InvalidAgent");
        };
        assert!(reason.contains("transport local only"), "{reason}");
    }

    const LOCAL: &str = "- id: helper\n  name: Helper\n  transport: local\n  agent: echo\n";

    /// Without the feature there is no local runtime to serve the entry, so it is refused at
    /// startup, naming the feature and the agent (never dropped, never sent to the A2A client).
    #[cfg(not(feature = "agent-local"))]
    #[test]
    fn a_local_agent_is_refused_naming_the_feature_in_a_build_without_it() {
        let err = agents_err(LOCAL, &[]);
        assert!(
            matches!(
                &err,
                ConfigError::LocalAgentsNotCompiled { agent, feature }
                    if agent == "helper" && *feature == "agent-local"
            ),
            "{err:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains("\"agent-local\"")
                && text.contains("--features agent-local")
                && text.contains("helper"),
            "{text}"
        );
        assert!(LocalAgentKind::ALL.iter().all(|k| !k.compiled_in()));
    }

    /// With the feature the same entry is accepted, as a local endpoint of its kind.
    #[cfg(feature = "agent-local")]
    #[test]
    fn a_local_agent_is_accepted_in_a_build_with_the_feature() {
        let entries = parse_agents(LOCAL, Path::new("agents.yaml"), env_of(&[])).unwrap();
        assert_eq!(
            entries[0].endpoint,
            AgentEndpoint::local(AgentId::new("helper"), "echo")
        );
        assert!(LocalAgentKind::ALL.iter().all(|k| k.compiled_in()));
    }

    #[test]
    fn no_local_kind_needs_a_model_yet() {
        assert!(LocalAgentKind::ALL.iter().all(|k| !k.needs_model()));
    }

    #[cfg(feature = "agent-local")]
    #[test]
    fn the_local_concurrency_defaults_to_four_and_must_be_positive() {
        assert_eq!(load(&base(), AGENTS).unwrap().agent_local_concurrency, 4);
        let mut env = base();
        env.push(("AGENT_LOCAL_CONCURRENCY", "9"));
        assert_eq!(load(&env, AGENTS).unwrap().agent_local_concurrency, 9);
        for bad in ["0", "many"] {
            let mut env = base();
            env.push(("AGENT_LOCAL_CONCURRENCY", bad));
            assert!(
                matches!(
                    load(&env, AGENTS).unwrap_err(),
                    ConfigError::Invalid {
                        var: "AGENT_LOCAL_CONCURRENCY",
                        ..
                    }
                ),
                "{bad}"
            );
        }
    }

    #[cfg(feature = "agent-local")]
    #[test]
    fn the_kinds_in_use_are_listed_once_in_file_order() {
        let both = format!("{AGENTS}{LOCAL}{}", LOCAL.replace("helper", "second"));
        let cfg = load(&base(), &both).unwrap();
        assert_eq!(cfg.local_kinds(), [LocalAgentKind::Echo]);
        assert!(load(&base(), AGENTS).unwrap().local_kinds().is_empty());
    }

    #[test]
    fn a_local_agent_becomes_a_local_endpoint_in_a_build_that_has_its_kind() {
        let entries =
            parse_agents_with(LOCAL, Path::new("agents.yaml"), env_of(&[]), |_| true).unwrap();
        assert_eq!(entries[0].name, "Helper");
        assert_eq!(
            entries[0].endpoint,
            AgentEndpoint::local(AgentId::new("helper"), "echo")
        );
        // It sits in file order beside a remote agent, and both are kept.
        let both =
            format!("- id: coder\n  name: Coder\n  cardUrl: https://c.example.com/card\n{LOCAL}");
        let entries =
            parse_agents_with(&both, Path::new("agents.yaml"), env_of(&[]), |_| true).unwrap();
        let ids: Vec<&str> = entries.iter().map(|e| e.endpoint.id.as_str()).collect();
        assert_eq!(ids, ["coder", "helper"]);
    }

    #[test]
    fn a_local_agent_needs_a_known_kind_and_takes_no_card_or_token() {
        let reason_of = |yaml: &str| {
            let err = parse_agents_with(yaml, Path::new("agents.yaml"), env_of(&[]), |_| true)
                .unwrap_err();
            let ConfigError::InvalidAgent { reason, .. } = err else {
                panic!("expected InvalidAgent, got {err:?}");
            };
            reason
        };
        let head = "- id: h\n  name: H\n  transport: local\n";
        let no_kind = reason_of(head);
        assert!(
            no_kind.contains("agent is required") && no_kind.contains("echo"),
            "{no_kind}"
        );
        let unknown = reason_of(&format!("{head}  agent: gpt\n"));
        assert!(
            unknown.contains("unknown local agent") && unknown.contains("echo"),
            "{unknown}"
        );
        for extra in ["  cardUrl: https://h.example.com/card\n", "  tokenEnv: T\n"] {
            let reason = reason_of(&format!("{head}  agent: echo\n{extra}"));
            assert!(reason.contains("transport a2a only"), "{reason}");
        }
    }

    #[test]
    fn an_empty_agent_list_is_refused() {
        for yaml in ["", "[]", "# nothing here\n"] {
            assert!(matches!(
                agents_err(yaml, &[]),
                ConfigError::NoAgents { .. }
            ));
        }
    }

    #[test]
    fn required_variables_are_reported_by_name() {
        let mut env = base();
        env.retain(|(k, _)| *k != "DATABASE_URL");
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::Missing("DATABASE_URL")
        ));
        let mut env = base();
        env.retain(|(k, _)| *k != "AGENTS_FILE");
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::Missing("AGENTS_FILE")
        ));
        // Set but empty is unset.
        let mut env = base();
        env.retain(|(k, _)| *k != "DATABASE_URL");
        env.push(("DATABASE_URL", ""));
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::Missing("DATABASE_URL")
        ));
    }

    #[test]
    fn an_unreadable_agents_file_is_an_error_naming_the_path() {
        let err = Config::load(args_of(&base()), env_of(&base()), |_| {
            Err(io::Error::new(io::ErrorKind::NotFound, "no such file"))
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::AgentsFileRead { .. }));
        assert!(err.to_string().contains("/etc/orch/agents.yaml"));
        // The cause is the source, printed once by the chain and not by the message.
        assert!(!err.to_string().contains("no such file"));
        let chain = orch_core::report(&err);
        assert_eq!(chain.matches("no such file").count(), 1, "{chain}");
    }

    #[test]
    fn numbers_and_addresses_are_validated() {
        let with = |k: &'static str, v: &'static str| {
            let mut env = base();
            env.push((k, v));
            load(&env, AGENTS)
        };
        let cfg = with("LISTEN_ADDR", "127.0.0.1:9090").unwrap();
        assert_eq!(cfg.listen_addr.port(), 9090);
        for (var, value) in [
            ("LISTEN_ADDR", "localhost"),
            ("DATABASE_MAX_CONNECTIONS", "1"),
            ("DATABASE_MAX_CONNECTIONS", "many"),
            ("DISPATCHER_CONCURRENCY", "0"),
            ("OUTBOX_LEASE_SECS", "2"),
            ("ORCH_VERIFIER_WATCH_SECS", "0"),
            ("ORCH_VERIFIER_WATCH_SECS", "often"),
            ("INBOX_LEASE_SECS", "2"),
            ("INBOX_POLL_SECS", "0"),
            ("INBOX_PARKED_TTL_SECS", "0"),
            ("INBOX_MAX_ATTEMPTS", "0"),
            ("INBOX_MAX_ATTEMPTS", "ten"),
            ("SHUTDOWN_GRACE_SECS", "0"),
        ] {
            let err = with(var, value).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { var: v, .. } if *v == var),
                "{var}={value}: {err}"
            );
        }
        assert_eq!(
            with("DATABASE_MAX_CONNECTIONS", "2")
                .unwrap()
                .database_max_connections,
            2
        );
    }

    #[test]
    fn the_dev_user_is_optional_and_must_be_an_email() {
        let mut env = base();
        env.push(("AUTH_DEV_USER", " Me@Example.com "));
        let cfg = load(&env, AGENTS).unwrap();
        assert_eq!(cfg.auth_dev_user.unwrap().as_str(), "me@example.com");
        let mut env = base();
        env.push(("AUTH_DEV_USER", "me"));
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::Invalid {
                var: "AUTH_DEV_USER",
                ..
            }
        ));
    }

    #[test]
    fn the_instance_id_comes_from_the_environment_or_the_hostname() {
        let mut env = base();
        env.push(("ORCH_INSTANCE_ID", "pod-a"));
        assert_eq!(load(&env, AGENTS).unwrap().instance_id, "pod-a");
        let mut env = base();
        env.push(("HOSTNAME", "orch-0"));
        assert!(
            load(&env, AGENTS)
                .unwrap()
                .instance_id
                .starts_with("orch-0-")
        );
    }

    #[test]
    fn debug_output_never_contains_secrets() {
        let cfg = load(&base(), AGENTS).unwrap();
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(!shown.contains("tok-123"), "{shown}");
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn debug_output_shows_how_many_webhook_secrets_and_never_which() {
        let cfg = load(
            &with(&[
                ("ORCH_SURFACES", "webhook-generic"),
                (
                    "WEBHOOK_GENERIC_SECRETS",
                    "hunter2-current-0123456789abcdef0123456789,hunter2-previous-0123456789abcdef0123456789",
                ),
            ]),
            AGENTS,
        )
        .unwrap();
        let shown = format!("{cfg:?}");
        assert!(shown.contains("<2 redacted>"), "{shown}");
        assert!(!shown.contains("hunter2"), "{shown}");
    }

    #[test]
    fn the_log_format_defaults_to_json() {
        assert_eq!(LogFormat::parse(None), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some("json")), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some("weird")), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some(" Text ")), LogFormat::Text);
    }

    #[cfg(feature = "surface-agui")]
    #[test]
    fn the_default_surface_is_agui_alone() {
        let agui = vec![Surface::Agui];
        let cfg = load(&base(), AGENTS).unwrap();
        assert_eq!(cfg.surfaces, agui);
        // A blank value is unset, as for every variable.
        let mut env = base();
        env.push(("ORCH_SURFACES", "  "));
        assert_eq!(load(&env, AGENTS).unwrap().surfaces, agui);
    }

    #[cfg(feature = "surface-agui")]
    #[test]
    fn surfaces_are_a_comma_list_of_known_names() {
        let with = |value: &'static str| {
            let mut env = base();
            env.push(("ORCH_SURFACES", value));
            load(&env, AGENTS)
        };
        assert_eq!(with("agui").unwrap().surfaces, vec![Surface::Agui]);
        assert_eq!(
            with(" agui ,").unwrap().surfaces,
            vec![Surface::Agui],
            "whitespace and a trailing comma are tolerated"
        );
        assert!(matches!(
            with("agui,agui").unwrap_err(),
            ConfigError::Invalid {
                var: "ORCH_SURFACES",
                ..
            }
        ));
        // The first bad name is the one reported.
        let err = with("agui,a2a,nope").unwrap_err();
        assert!(matches!(&err, ConfigError::UnknownSurface { name, .. } if name == "a2a"));
    }

    /// The legacy chat API was removed on 2026-09-30 (ADR 0012). A deployment that still lists it
    /// must not start quietly without the routes it expects: the error names the surface, says it
    /// is gone, and points to AG-UI. It is a `ConfigError`, so the process exits 78.
    #[test]
    fn the_removed_chat_api_surface_fails_closed_and_points_to_agui() {
        for value in ["chat-api", "agui,chat-api", "chat-api,agui", " chat-api "] {
            let mut env = base();
            env.push(("ORCH_SURFACES", value));
            let err = load(&env, AGENTS).unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConfigError::RemovedSurface {
                        name: "chat-api",
                        ..
                    }
                ),
                "{value:?}: {err}"
            );
            let shown = err.to_string();
            assert!(shown.contains("ORCH_SURFACES"), "{shown}");
            assert!(
                shown.contains("\"chat-api\" was removed on 2026-09-30"),
                "{shown}"
            );
            assert!(shown.contains("Use AG-UI instead"), "{shown}");
            assert!(shown.contains("POST /agui/agents/{agentId}"), "{shown}");
            assert!(shown.contains("docs/api/agui.md"), "{shown}");
        }
        // The flag is checked the same way as the variable.
        let mut args = args_of(&base());
        args.surfaces = Some("agui,chat-api".to_owned());
        let err = Config::load(args, env_of(&base()), |_| Ok(AGENTS.to_owned())).unwrap_err();
        assert!(matches!(err, ConfigError::RemovedSurface { .. }), "{err}");
        // A different spelling is not the removed surface: names are exact.
        let mut env = base();
        env.push(("ORCH_SURFACES", "CHAT-API"));
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::UnknownSurface { .. }
        ));
    }

    #[test]
    fn an_unknown_surface_is_refused_naming_the_variable_and_the_known_ones() {
        let mut env = base();
        env.push(("ORCH_SURFACES", "a2a"));
        let err = load(&env, AGENTS).unwrap_err();
        assert!(matches!(&err, ConfigError::UnknownSurface { name, .. } if name == "a2a"));
        let shown = err.to_string();
        assert!(shown.contains("ORCH_SURFACES"), "{shown}");
        assert!(shown.contains("agui"), "names what is known: {shown}");
    }

    #[test]
    fn an_empty_surface_list_is_refused() {
        for value in [",", " , ,"] {
            let mut env = base();
            env.push(("ORCH_SURFACES", value));
            let err = load(&env, AGENTS).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { var: "ORCH_SURFACES", reason }
                    if reason.contains("empty")),
                "{value:?}: {err}"
            );
        }
    }

    #[test]
    fn every_known_surface_names_its_feature() {
        for surface in Surface::ALL {
            assert!(surface.feature().starts_with("surface-"), "{surface}");
            assert_eq!(surface.to_string(), surface.name());
        }
    }

    #[cfg(feature = "surface-agui")]
    #[test]
    fn the_agui_surface_is_compiled_in_by_its_feature() {
        assert!(Surface::Agui.compiled_in());
        assert_eq!(Surface::Agui.feature(), "surface-agui");
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn the_generic_webhook_is_mounted_by_name_with_its_secrets() {
        let cfg = load(
            &with(&[
                ("ORCH_SURFACES", "agui,webhook-generic"),
                (
                    "WEBHOOK_GENERIC_SECRETS",
                    " dev-webhook-secret-0123456789abcdef0123 ",
                ),
            ]),
            AGENTS,
        )
        .unwrap();
        assert_eq!(cfg.surfaces, vec![Surface::Agui, Surface::WebhookGeneric]);
        let generic = cfg.webhook_generic.expect("configured");
        assert_eq!(generic.secrets.len(), 1);
        assert_eq!(
            generic.max_skew,
            Duration::from_secs(300),
            "the default skew"
        );
        assert_eq!(Surface::WebhookGeneric.name(), "webhook-generic");
        assert_eq!(Surface::WebhookGeneric.feature(), "surface-webhook");
        // It is not a default surface: the webhook must be asked for.
        let cfg = load(&base(), AGENTS).unwrap();
        assert!(!cfg.surfaces.contains(&Surface::WebhookGeneric));
        assert!(cfg.webhook_generic.is_none());
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn the_github_webhook_is_mounted_by_name_with_its_own_secrets() {
        let cfg = load(
            &with(&[
                ("ORCH_SURFACES", "agui,webhook-generic,webhook-github"),
                ("WEBHOOK_GENERIC_SECRETS", "generic-secret-0123456789abcdef0123456789"),
                (
                    "WEBHOOK_GITHUB_SECRETS",
                    "github-secret-new-0123456789abcdef0123456789, github-secret-old-0123456789abcdef0123456789",
                ),
            ]),
            AGENTS,
        )
        .unwrap();
        assert_eq!(
            cfg.surfaces,
            vec![
                Surface::Agui,
                Surface::WebhookGeneric,
                Surface::WebhookGithub
            ]
        );
        assert_eq!(cfg.webhook_github.as_ref().unwrap().secrets.len(), 2);
        assert_eq!(cfg.webhook_generic.as_ref().unwrap().secrets.len(), 1);
        assert_eq!(Surface::WebhookGithub.name(), "webhook-github");
        assert_eq!(Surface::WebhookGithub.feature(), "surface-webhook");
        // Each route needs its own secret: the generic one's is not the GitHub one's.
        let err = load(
            &with(&[
                ("ORCH_SURFACES", "webhook-github"),
                (
                    "WEBHOOK_GENERIC_SECRETS",
                    "generic-secret-0123456789abcdef0123456789",
                ),
            ]),
            AGENTS,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "WEBHOOK_GITHUB_SECRETS is required when ORCH_SURFACES mounts \"webhook-github\""
        );
        // Mounting the GitHub route alone needs nothing of the generic one.
        let cfg = load(
            &with(&[
                ("ORCH_SURFACES", "webhook-github"),
                (
                    "WEBHOOK_GITHUB_SECRETS",
                    "github-secret-0123456789abcdef0123456789",
                ),
            ]),
            AGENTS,
        )
        .unwrap();
        assert!(cfg.webhook_generic.is_none());
        assert!(cfg.webhook_github.is_some());
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn the_github_secrets_are_checked_redacted_and_not_needed_by_a_worker() {
        for (secrets, says) in [
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,hunter2-c-0123456789abcdef0123456789",
                "at most 2",
            ),
            (" , ", "names no secret"),
        ] {
            let err = load(&with(&[("WEBHOOK_GITHUB_SECRETS", secrets)]), AGENTS).unwrap_err();
            assert!(!err.to_string().contains("hunter2"), "{err}");
            let (var, reason) = invalid_var(err);
            assert_eq!(var, "WEBHOOK_GITHUB_SECRETS");
            assert!(reason.contains(says), "{reason}");
        }
        let cfg = load(
            &with(&[
                ("ORCH_SURFACES", "webhook-github"),
                (
                    "WEBHOOK_GITHUB_SECRETS",
                    "hunter2-current-0123456789abcdef0123456789",
                ),
            ]),
            AGENTS,
        )
        .unwrap();
        let shown = format!("{cfg:?}");
        assert!(
            shown.contains("webhook_github") && shown.contains("<1 redacted>"),
            "{shown}"
        );
        assert!(!shown.contains("hunter2"), "{shown}");
        // A worker serves no routes.
        let worker = load(
            &with(&[("ORCH_SURFACES", "webhook-github"), ("ORCH_ROLE", "worker")]),
            AGENTS,
        )
        .unwrap();
        assert!(worker.webhook_github.is_none());
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn a_mounted_webhook_without_secrets_is_refused_and_fails_closed() {
        for secrets in [None, Some(""), Some("  "), Some(" , ,")] {
            let mut pairs = vec![("ORCH_SURFACES", "webhook-generic")];
            if let Some(secrets) = secrets {
                pairs.push(("WEBHOOK_GENERIC_SECRETS", secrets));
            }
            let err = load(&with(&pairs), AGENTS).unwrap_err();
            match (&err, secrets) {
                // Unset or blank counts as unset: required.
                (
                    ConfigError::MissingForSurface {
                        var: "WEBHOOK_GENERIC_SECRETS",
                        surface: "webhook-generic",
                    },
                    None | Some("" | "  "),
                ) => {}
                // A value that is set but names no secret is invalid.
                (
                    ConfigError::Invalid {
                        var: "WEBHOOK_GENERIC_SECRETS",
                        reason,
                    },
                    Some(" , ,"),
                ) => {
                    assert!(reason.contains("names no secret"), "{reason}");
                }
                _ => panic!("{secrets:?}: {err}"),
            }
            assert!(
                err.to_string().contains("WEBHOOK_GENERIC_SECRETS"),
                "{secrets:?}: {err}"
            );
        }
        let err = load(&with(&[("ORCH_SURFACES", "agui,webhook-generic")]), AGENTS).unwrap_err();
        assert_eq!(
            err.to_string(),
            "WEBHOOK_GENERIC_SECRETS is required when ORCH_SURFACES mounts \"webhook-generic\""
        );
    }

    /// A worker serves no routes, so it does not need the secret of a route it will not mount
    /// (the environment of the control plane may be shared with it).
    #[cfg(feature = "surface-webhook")]
    #[test]
    fn a_worker_does_not_need_the_secret_of_a_route_it_does_not_mount() {
        let pairs = [
            ("ORCH_SURFACES", "webhook-generic"),
            ("ORCH_ROLE", "worker"),
        ];
        let cfg = load(&with(&pairs), AGENTS).unwrap();
        assert!(cfg.webhook_generic.is_none());
        // A control plane does.
        let pairs = [
            ("ORCH_SURFACES", "webhook-generic"),
            ("ORCH_ROLE", "control-plane"),
        ];
        assert!(matches!(
            load(&with(&pairs), AGENTS).unwrap_err(),
            ConfigError::MissingForSurface { .. }
        ));
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn the_webhook_settings_are_checked_and_a_secret_is_never_echoed() {
        let cases = [
            (
                vec![
                    (
                        "WEBHOOK_GENERIC_SECRETS",
                        "hunter2-a-0123456789abcdef0123456789,hunter2-b-0123456789abcdef0123456789,hunter2-c-0123456789abcdef0123456789",
                    ),
                    ("ORCH_SURFACES", "webhook-generic"),
                ],
                "WEBHOOK_GENERIC_SECRETS",
                "at most 2",
            ),
            (
                vec![
                    ("WEBHOOK_GENERIC_SECRETS", "hunter2"),
                    ("WEBHOOK_GENERIC_MAX_SKEW_SECS", "0"),
                ],
                "WEBHOOK_GENERIC_MAX_SKEW_SECS",
                "at least 1",
            ),
            (
                vec![
                    ("WEBHOOK_GENERIC_SECRETS", "hunter2"),
                    ("WEBHOOK_GENERIC_MAX_SKEW_SECS", "five minutes"),
                ],
                "WEBHOOK_GENERIC_MAX_SKEW_SECS",
                "not a number",
            ),
            // Set, but not mounted (agui alone): still checked, so a typo shows at once.
            (
                vec![(
                    "WEBHOOK_GENERIC_SECRETS",
                    "hunter2-a-0123456789abcdef0123456789,hunter2-b-0123456789abcdef0123456789,hunter2-c-0123456789abcdef0123456789",
                )],
                "WEBHOOK_GENERIC_SECRETS",
                "at most 2",
            ),
        ];
        for (pairs, var, says) in cases {
            let err = load(&with(&pairs), AGENTS).unwrap_err();
            assert!(!err.to_string().contains("hunter2"), "{err}");
            let (got, reason) = invalid_var(err);
            assert_eq!(got, var, "{pairs:?}");
            assert!(reason.contains(says), "{pairs:?}: {reason}");
        }
        let cfg = load(
            &with(&[
                ("WEBHOOK_GENERIC_SECRETS", "new-new-new-new-new-new-new-new-new-new,old-old-old-old-old-old-old-old-old-old"),
                ("WEBHOOK_GENERIC_MAX_SKEW_SECS", "60"),
                ("ORCH_SURFACES", "webhook-generic"),
            ]),
            AGENTS,
        )
        .unwrap();
        let generic = cfg.webhook_generic.unwrap();
        assert_eq!(generic.secrets.len(), 2);
        assert_eq!(generic.max_skew, Duration::from_secs(60));
    }

    #[cfg(not(feature = "surface-webhook"))]
    #[test]
    fn the_webhook_that_is_not_compiled_in_is_refused_naming_its_feature() {
        let err = load(&with(&[("ORCH_SURFACES", "webhook-generic")]), AGENTS).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::SurfaceNotCompiled {
                surface: "webhook-generic",
                feature: "surface-webhook"
            }
        ));
    }

    #[cfg(not(feature = "surface-agui"))]
    #[test]
    fn agui_that_is_not_compiled_in_is_refused_naming_its_feature() {
        let mut env = base();
        env.push(("ORCH_SURFACES", "agui"));
        let err = load(&env, AGENTS).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::SurfaceNotCompiled {
                surface: "agui",
                feature: "surface-agui"
            }
        ));
        assert!(err.to_string().contains("surface-agui"), "{err}");
    }

    #[test]
    fn the_role_defaults_to_all() {
        assert_eq!(load(&base(), AGENTS).unwrap().role, Role::All);
        // Blank is unset, like every other variable.
        for blank in ["", "  "] {
            let mut pairs = base();
            pairs.push(("ORCH_ROLE", blank));
            assert_eq!(load(&pairs, AGENTS).unwrap().role, Role::All, "{blank:?}");
        }
    }

    #[test]
    fn every_role_is_accepted_by_its_name() {
        for (name, role) in [
            ("all", Role::All),
            ("control-plane", Role::ControlPlane),
            ("worker", Role::Worker),
            (" Worker ", Role::Worker),
            ("CONTROL-PLANE", Role::ControlPlane),
        ] {
            let mut pairs = base();
            pairs.push(("ORCH_ROLE", name));
            assert_eq!(load(&pairs, AGENTS).unwrap().role, role, "{name:?}");
        }
        // The names are adam-host's: every one of its roles is one this binary can run.
        assert_eq!(Role::VALUES.len(), 3);
    }

    #[test]
    fn an_unknown_role_is_a_config_error_naming_the_variable_and_the_choices() {
        for bad in ["controlplane", "control_plane", "workers", "cp", "x"] {
            let mut pairs = base();
            pairs.push(("ORCH_ROLE", bad));
            let err = load(&pairs, AGENTS).unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        var: "ORCH_ROLE",
                        ..
                    }
                ),
                "{bad}: {err}"
            );
            let msg = err.to_string();
            assert!(msg.starts_with("ORCH_ROLE is invalid"), "{msg}");
            assert!(
                msg.contains("all") && msg.contains("control-plane") && msg.contains("worker"),
                "the message lists the accepted values: {msg}"
            );
        }
    }

    #[test]
    fn the_role_flag_is_collected_raw_and_validated_by_load() {
        // Clap only collects the string, so a bad value is the same error from any source
        // (the smoke tests cover the flag beating its variable in the real binary).
        let args = Args::try_parse_from(["orchestrator", "--role", "worker"]).unwrap();
        assert_eq!(args.role.as_deref(), Some("worker"));
        let args = Args::try_parse_from(["orchestrator", "--role", "nowhere"]).unwrap();
        assert_eq!(args.role.as_deref(), Some("nowhere"));
        let mut args = args_of(&base());
        args.role = Some("control-plane".to_owned());
        let cfg = Config::load(args, env_of(&base()), |_| Ok(AGENTS.to_owned())).unwrap();
        assert_eq!(cfg.role, Role::ControlPlane);
    }

    #[test]
    fn flags_are_parsed_by_clap_and_win_over_the_environment() {
        let args = Args::try_parse_from([
            "orchestrator",
            "--database-url",
            "postgres://flag/db",
            "--agents-file",
            "/etc/agents.yaml",
            "--listen-addr",
            "127.0.0.1:9000",
            "--surfaces",
            "agui",
            "--database-max-connections",
            "4",
        ])
        .unwrap();
        assert_eq!(args.database_url.as_deref(), Some("postgres://flag/db"));
        assert_eq!(args.listen_addr.as_deref(), Some("127.0.0.1:9000"));
        assert_eq!(args.surfaces.as_deref(), Some("agui"));
        assert_eq!(args.database_max_connections.as_deref(), Some("4"));
        // Nothing here is validated by clap: a bad value is a ConfigError, exit code 78.
        let args = Args::try_parse_from(["orchestrator", "--listen-addr", "nowhere"]).unwrap();
        assert_eq!(args.listen_addr.as_deref(), Some("nowhere"));
        assert!(Args::try_parse_from(["orchestrator", "--no-such-flag"]).is_err());
    }

    #[test]
    fn help_lists_every_variable_the_service_has_always_read() {
        use clap::CommandFactory;
        let help = Args::command().render_long_help().to_string();
        for var in [
            "DATABASE_URL",
            "AGENTS_FILE",
            "LISTEN_ADDR",
            "ORCH_ROLE",
            "ORCH_SURFACES",
            "ORCH_GATE",
            "ORCH_MAX_ATTEMPTS",
            "ORCH_MAX_ATTEMPTS_CAP",
            "ORCH_VERIFIER",
            "MCP_TOKENS_FILE",
            "MCP_ALLOWED_HOSTS",
            "ORCH_PUBLIC_URL",
            "MCP_WAIT_MAX_SECS",
            "MCP_WAIT_MAX_CONCURRENT",
            "MCP_WAIT_MAX_PER_USER",
            "MCP_ALLOWED_ORIGINS",
            "THREAD_TOOLS_SECRET",
            "THREAD_TOOLS_SECRET_PREVIOUS",
            "THREAD_TOOLS_URL",
            "THREAD_TOOLS_TOKEN_TTL_SECS",
            "THREAD_TOOLS_ALLOWED_HOSTS",
            "ORCH_VERIFIER_TIMEOUT_SECS",
            "ORCH_VERIFIER_WATCH_SECS",
            "AUTH_DEV_USER",
            "DATABASE_MAX_CONNECTIONS",
            "DISPATCHER_CONCURRENCY",
            "OUTBOX_LEASE_SECS",
            "INBOX_LEASE_SECS",
            "INBOX_POLL_SECS",
            "INBOX_PARKED_TTL_SECS",
            "INBOX_MAX_ATTEMPTS",
            "SHUTDOWN_GRACE_SECS",
            "ORCH_INSTANCE_ID",
            "LOG_FORMAT",
        ] {
            assert!(help.contains(var), "--help does not mention {var}:\n{help}");
        }
        assert!(help.contains("--surfaces"), "{help}");
        assert!(help.contains("--role"), "{help}");
        for name in ["all", "control-plane", "worker"] {
            assert!(
                help.contains(name),
                "--help does not name the role {name}:\n{help}"
            );
        }
    }

    // ---- the verification gate (ADR 0018) --------------------------------------------------

    fn with(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        let mut pairs = base();
        pairs.extend_from_slice(extra);
        pairs
    }

    /// The environment of a deployment that receives CI reports: the generic webhook mounted, with a
    /// secret, and `extra` on top.
    #[cfg(feature = "surface-webhook")]
    fn with_ci_surface(
        extra: &[(&'static str, &'static str)],
    ) -> Vec<(&'static str, &'static str)> {
        let mut pairs = base();
        pairs.extend_from_slice(&[
            ("ORCH_SURFACES", "agui,webhook-generic"),
            (
                "WEBHOOK_GENERIC_SECRETS",
                "dev-webhook-secret-0123456789abcdef0123",
            ),
        ]);
        pairs.extend_from_slice(extra);
        pairs
    }

    fn invalid_var(err: ConfigError) -> (&'static str, String) {
        match err {
            ConfigError::Invalid { var, reason } => (var, reason),
            other => panic!("expected an invalid variable, got {other}"),
        }
    }

    /// `AGENTS` with a `gate:` on the first agent.
    fn agents_with_gate(gate: &str) -> String {
        format!(
            "\
- id: coder
  name: Coder
  cardUrl: https://coder.example.com/.well-known/agent-card.json
  tokenEnv: CODER_A2A_TOKEN
  gate: {gate}
- id: plain
  name: Plain
  cardUrl: http://plain.internal:9000/.well-known/agent-card.json
"
        )
    }

    #[test]
    fn the_gate_defaults_to_none_with_three_attempts_and_a_cap_of_ten() {
        let cfg = load(&base(), AGENTS).unwrap();
        assert!(cfg.gate.require.is_empty(), "no gate: today's behaviour");
        assert!(!cfg.gate.is_active());
        assert_eq!(cfg.gate.max_attempts, 3);
        assert_eq!(cfg.gate_rules.cap(), 10);
        assert!(cfg.target_gates.is_empty());
        let app = cfg.app_config();
        assert_eq!(app.gate, cfg.gate);
        assert_eq!(app.gate_rules, cfg.gate_rules);
    }

    #[test]
    fn the_deployment_gate_comes_from_the_environment() {
        let cfg = load(
            &with(&[
                ("ORCH_GATE", " agent-checks , agent-checks"),
                ("ORCH_MAX_ATTEMPTS", "5"),
                ("ORCH_MAX_ATTEMPTS_CAP", "6"),
            ]),
            AGENTS,
        )
        .unwrap();
        assert_eq!(cfg.gate.require, [CheckSource::AgentChecks].into());
        assert_eq!(cfg.gate.max_attempts, 5);
        assert_eq!(cfg.gate_rules.cap(), 6);
    }

    #[test]
    fn the_verifier_comes_from_the_environment_with_its_own_timeout() {
        // The deployment requires the verifier of everyone; `plain` is the verifier, and its own
        // entry leaves the source out for itself, the one removal allowed.
        let agents = "\
- id: coder
  name: Coder
  cardUrl: https://coder.example.com/.well-known/agent-card.json
  tokenEnv: CODER_A2A_TOKEN
- id: plain
  name: Plain
  cardUrl: http://plain.internal:9000/.well-known/agent-card.json
  gate: {require: []}
";
        let cfg = load(
            &with(&[
                ("ORCH_GATE", "verifier"),
                ("ORCH_VERIFIER", "plain"),
                ("ORCH_VERIFIER_TIMEOUT_SECS", "90"),
                ("ORCH_VERIFIER_WATCH_SECS", "2"),
            ]),
            agents,
        )
        .unwrap();
        assert_eq!(cfg.verifier_watch, Duration::from_secs(2));
        assert_eq!(cfg.gate.require, [CheckSource::Verifier].into());
        assert_eq!(cfg.gate.verifier, Some(AgentId::new("plain")));
        assert_eq!(cfg.gate.verifier_timeout.as_secs(), 90);
        assert_eq!(cfg.app_config().gate, cfg.gate);

        // The default wait is half an hour, with or without a verifier.
        let cfg = load(&base(), AGENTS).unwrap();
        assert_eq!(cfg.gate.verifier_timeout.as_secs(), 1800);
        assert!(cfg.gate.verifier.is_none());
        // A verifier that is named but not required is fine, and is not a self-verification.
        let cfg = load(&with(&[("ORCH_VERIFIER", "plain")]), AGENTS).unwrap();
        assert_eq!(cfg.gate.verifier, Some(AgentId::new("plain")));
        assert!(!cfg.gate.is_active());
    }

    #[test]
    fn a_verifier_that_cannot_work_is_refused_at_startup() {
        for (pairs, var, says) in [
            (
                vec![("ORCH_GATE", "verifier")],
                None,
                "no verifier agent is configured",
            ),
            (
                vec![("ORCH_GATE", "verifier"), ("ORCH_VERIFIER", "ghost")],
                Some("ORCH_VERIFIER"),
                "not a configured agent",
            ),
            (
                vec![("ORCH_VERIFIER", "ghost")],
                Some("ORCH_VERIFIER"),
                "not a configured agent",
            ),
            (
                vec![("ORCH_VERIFIER_TIMEOUT_SECS", "0")],
                Some("ORCH_VERIFIER_TIMEOUT_SECS"),
                "at least 1",
            ),
            (
                vec![("ORCH_VERIFIER_TIMEOUT_SECS", "soon")],
                Some("ORCH_VERIFIER_TIMEOUT_SECS"),
                "not a number",
            ),
        ] {
            let err = load(&with(&pairs), AGENTS).unwrap_err();
            match (var, err) {
                (Some(var), err) => {
                    let (got, reason) = invalid_var(err);
                    assert_eq!(got, var, "{pairs:?}");
                    assert!(reason.contains(says), "{pairs:?}: {reason}");
                }
                (None, ConfigError::Gate { reason, .. }) => {
                    assert!(reason.contains(says), "{pairs:?}: {reason}");
                }
                (None, other) => panic!("{pairs:?}: {other}"),
            }
        }
        // The deployment requires the verifier of everyone, the verifier included: it would
        // verify its own work, and the message says how to fix it.
        let err = load(
            &with(&[("ORCH_GATE", "verifier"), ("ORCH_VERIFIER", "plain")]),
            AGENTS,
        )
        .unwrap_err();
        let ConfigError::Gate { reason, .. } = &err else {
            panic!("{err}");
        };
        assert!(reason.contains("would verify its own work"), "{reason}");
        assert!(reason.contains("leaves out `verifier`"), "{reason}");
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn ci_is_accepted_as_a_source_and_its_timeout_is_read() {
        let cfg = load(
            &with_ci_surface(&[("ORCH_GATE", "ci"), ("ORCH_CI_REQUIRED", "build, lint")]),
            AGENTS,
        )
        .unwrap();
        assert_eq!(cfg.gate.require, [CheckSource::Ci].into());
        assert_eq!(
            cfg.gate.ci.required,
            ["build".to_owned(), "lint".to_owned()].into()
        );
        assert_eq!(cfg.gate.ci.timeout, jiff::SignedDuration::from_secs(3600));
        let cfg = load(
            &with_ci_surface(&[
                ("ORCH_GATE", "agent-checks, ci"),
                ("ORCH_CI_REQUIRED", "build"),
                ("ORCH_CI_TIMEOUT_SECS", "90"),
            ]),
            AGENTS,
        )
        .unwrap();
        assert_eq!(cfg.gate.ci.timeout, jiff::SignedDuration::from_secs(90));
        assert_eq!(cfg.app_config().gate.ci.timeout.as_secs(), 90);
        // The timeout is read without a ci gate too: an agent's own gate may require ci.
        let cfg = load(&with(&[("ORCH_CI_TIMEOUT_SECS", "5")]), AGENTS).unwrap();
        assert!(cfg.gate.require.is_empty());
        assert_eq!(cfg.gate.ci.timeout.as_secs(), 5);
    }

    #[test]
    fn a_bad_ci_timeout_names_itself() {
        for (bad, says) in [
            ("0", "at least 1"),
            ("-3", "at least 1"),
            ("soon", "not a number"),
        ] {
            let (var, reason) =
                invalid_var(load(&with(&[("ORCH_CI_TIMEOUT_SECS", bad)]), AGENTS).unwrap_err());
            assert_eq!(var, "ORCH_CI_TIMEOUT_SECS", "{bad}");
            assert!(reason.contains(says), "{bad}: {reason}");
        }
    }

    /// "The first report decides" lets a red commit pass on whichever report arrives first, so a
    /// gate that requires `ci` names its checks, in the deployment and in every agent's entry.
    #[cfg(feature = "surface-webhook")]
    #[test]
    fn a_gate_that_requires_ci_without_naming_a_check_is_refused_at_startup() {
        let err = load(&with_ci_surface(&[("ORCH_GATE", "ci")]), AGENTS).unwrap_err();
        let (var, reason) = invalid_var(err);
        assert_eq!(var, "ORCH_GATE", "the deployment's fault, not an agent's");
        assert!(reason.contains("ci.required"), "{reason}");
        for gate in [
            "{require: [ci]}",
            "{require: [ci], ci: {required: []}}",
            "{require: [ci], ci: {timeoutSecs: 30}}",
        ] {
            let err = load(&with_ci_surface(&[]), &agents_with_gate(gate)).unwrap_err();
            let ConfigError::Gate { context, reason } = &err else {
                panic!("{gate}: expected a gate error, got {err}");
            };
            assert_eq!(*context, "AGENTS_FILE");
            assert!(
                reason.contains("coder") && reason.contains("ci.required"),
                "{gate}: {reason}"
            );
        }
        // Names in the deployment serve every agent that adds ci.
        let cfg = load(
            &with_ci_surface(&[("ORCH_CI_REQUIRED", "build")]),
            &agents_with_gate("{require: [ci]}"),
        )
        .unwrap();
        assert_eq!(cfg.gate.ci.required, ["build".to_owned()].into());
        // A list of blanks names nothing.
        let err = load(&with_ci_surface(&[("ORCH_CI_REQUIRED", " , ")]), AGENTS).unwrap_err();
        // (an all-blank value is unset, a comma list of blanks is a mistake)
        assert_eq!(invalid_var(err).0, "ORCH_CI_REQUIRED");
    }

    /// With no CI webhook surface mounted, a report has no door: the process refuses `ci` in every
    /// layer, per-thread requests included (they are checked with the same rules).
    #[test]
    fn ci_is_refused_when_no_ci_webhook_surface_is_mounted() {
        let why = "no CI webhook surface is mounted (ORCH_SURFACES)";
        let err = load(
            &with(&[("ORCH_GATE", "ci"), ("ORCH_CI_REQUIRED", "build")]),
            AGENTS,
        )
        .unwrap_err();
        let (var, reason) = invalid_var(err);
        assert_eq!(var, "ORCH_GATE");
        assert!(reason.contains(why), "{reason}");
        // An agent's entry, and the ci settings of the deployment.
        let err = load(
            &base(),
            &agents_with_gate("{require: [ci], ci: {required: [build]}}"),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ConfigError::Gate { reason, .. } if reason.contains(why)),
            "{err}"
        );
        let err = load(&with(&[("ORCH_CI_REQUIRED", "build")]), AGENTS).unwrap_err();
        assert_eq!(invalid_var(err).0, "ORCH_CI_REQUIRED");
        // The rules the process keeps refuse it for a thread as well.
        let cfg = load(&base(), AGENTS).unwrap();
        assert!(!cfg.gate_rules.honours(CheckSource::Ci));
        let mut above = orch_core::GatePolicy::default();
        above.ci.required = ["build".to_owned()].into();
        let request = orch_app::GateLayer::from_json(&serde_json::json!({"require": ["ci"]}))
            .unwrap()
            .unwrap();
        let err = cfg
            .gate_rules
            .apply(&above, &request, &Layer::Thread)
            .unwrap_err();
        assert!(err.to_string().contains(why), "{err}");
        // A worker serves no routes and cannot tell: the control plane decides.
        let worker = load(
            &with(&[
                ("ORCH_ROLE", "worker"),
                ("ORCH_GATE", "ci"),
                ("ORCH_CI_REQUIRED", "build"),
            ]),
            AGENTS,
        )
        .unwrap();
        assert!(worker.gate_rules.honours(CheckSource::Ci));
        // A surface list with the GitHub webhook alone is a door too.
        #[cfg(feature = "surface-webhook")]
        {
            let cfg = load(
                &with(&[
                    ("ORCH_SURFACES", "webhook-github"),
                    (
                        "WEBHOOK_GITHUB_SECRETS",
                        "github-secret-0123456789abcdef0123456789",
                    ),
                    ("ORCH_GATE", "ci"),
                    ("ORCH_CI_REQUIRED", "build"),
                ]),
                AGENTS,
            )
            .unwrap();
            assert!(cfg.gate_rules.honours(CheckSource::Ci));
        }
    }

    #[test]
    fn a_bad_gate_variable_names_itself() {
        let cases = [
            (
                vec![("ORCH_GATE", "nonsense")],
                "ORCH_GATE",
                "one of: ci, agent-checks, verifier",
            ),
            (vec![("ORCH_GATE", ",")], "ORCH_GATE", "names no source"),
            (
                vec![("ORCH_MAX_ATTEMPTS", "0")],
                "ORCH_MAX_ATTEMPTS",
                "at least 1",
            ),
            (
                vec![("ORCH_MAX_ATTEMPTS", "many")],
                "ORCH_MAX_ATTEMPTS",
                "not a number",
            ),
            (
                vec![("ORCH_MAX_ATTEMPTS", "11")],
                "ORCH_MAX_ATTEMPTS",
                "above ORCH_MAX_ATTEMPTS_CAP (10)",
            ),
            (
                vec![("ORCH_MAX_ATTEMPTS_CAP", "0")],
                "ORCH_MAX_ATTEMPTS_CAP",
                "at least 1",
            ),
            (
                vec![("ORCH_MAX_ATTEMPTS_CAP", "2"), ("ORCH_MAX_ATTEMPTS", "3")],
                "ORCH_MAX_ATTEMPTS",
                "above ORCH_MAX_ATTEMPTS_CAP (2)",
            ),
            (
                vec![("ORCH_MAX_ATTEMPTS_CAP", "101")],
                "ORCH_MAX_ATTEMPTS_CAP",
                "above the ceiling of 100",
            ),
        ];
        for (pairs, var, says) in cases {
            let err = load(&with(&pairs), AGENTS).unwrap_err();
            let (got, reason) = invalid_var(err);
            assert_eq!(got, var, "{pairs:?}");
            assert!(reason.contains(says), "{pairs:?}: {reason}");
        }
    }

    #[test]
    fn a_target_gate_is_read_from_agents_file_and_checked_on_top_of_the_deployment() {
        let cfg = load(
            &with(&[("ORCH_GATE", "agent-checks")]),
            &agents_with_gate("{require: [agent-checks], maxAttempts: 2}"),
        )
        .unwrap();
        let layer = &cfg.target_gates[&AgentId::new("coder")];
        assert_eq!(layer.max_attempts, Some(2));
        assert!(!cfg.target_gates.contains_key(&AgentId::new("plain")));
        assert_eq!(cfg.app_config().target_gates, cfg.target_gates);

        // Without a deployment gate the entry alone gates its agent.
        let cfg = load(&base(), &agents_with_gate("{require: [agent-checks]}")).unwrap();
        assert!(cfg.gate.require.is_empty());
        assert_eq!(cfg.target_gates.len(), 1);
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn a_target_may_require_ci_and_its_timeout_overrides_the_deployments() {
        let cfg = load(
            &with_ci_surface(&[("ORCH_CI_TIMEOUT_SECS", "600")]),
            &agents_with_gate("{require: [ci], ci: {required: [build], timeoutSecs: 45}}"),
        )
        .unwrap();
        let rules = &cfg.gate_rules;
        let coder = AgentId::new("coder");
        let policy = rules
            .for_target(&cfg.gate, &coder, cfg.target_gates.get(&coder))
            .unwrap();
        assert!(policy.requires(CheckSource::Ci));
        assert_eq!(policy.ci.required, ["build".to_owned()].into());
        assert_eq!(policy.ci.timeout.as_secs(), 45);
        // The other agent keeps the deployment's.
        let plain = AgentId::new("plain");
        let policy = rules
            .for_target(&cfg.gate, &plain, cfg.target_gates.get(&plain))
            .unwrap();
        assert_eq!(policy.ci.timeout.as_secs(), 600);
    }

    #[test]
    fn a_target_gate_is_parsed_strictly() {
        for gate in [
            "{surprise: 1}",
            "{require: agent-checks}",
            "{maxAttempts: many}",
            "{ci: {surprise: 1}}",
            "just-a-string",
        ] {
            let err = load(&base(), &agents_with_gate(gate)).unwrap_err();
            assert!(
                matches!(err, ConfigError::AgentsFileParse { .. }),
                "{gate}: {err}"
            );
        }
    }

    #[test]
    fn a_target_may_not_weaken_the_deployment_or_exceed_the_cap() {
        let err = load(
            &with(&[("ORCH_GATE", "agent-checks")]),
            &agents_with_gate("{require: []}"),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ConfigError::Gate { reason, .. } if reason.contains("may add sources")),
            "{err}"
        );
        let err = load(&base(), &agents_with_gate("{maxAttempts: 11}")).unwrap_err();
        assert!(
            matches!(&err, ConfigError::Gate { reason, .. } if reason.contains("1..=10")),
            "{err}"
        );
        // The cap is the deployment's to raise.
        let cfg = load(
            &with(&[("ORCH_MAX_ATTEMPTS_CAP", "20")]),
            &agents_with_gate("{maxAttempts: 11}"),
        )
        .unwrap();
        assert_eq!(cfg.gate_rules.cap(), 20);
    }

    #[test]
    fn the_debug_output_shows_the_gate() {
        let cfg = load(&with(&[("ORCH_GATE", "agent-checks")]), AGENTS).unwrap();
        let shown = format!("{cfg:?}");
        assert!(shown.contains("AgentChecks"), "{shown}");
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn a_cap_below_the_default_lowers_the_default_attempts_with_it() {
        let cfg = load(&with(&[("ORCH_MAX_ATTEMPTS_CAP", "2")]), AGENTS).unwrap();
        assert_eq!((cfg.gate.max_attempts, cfg.gate_rules.cap()), (2, 2));
        let cfg = load(&with(&[("ORCH_MAX_ATTEMPTS_CAP", "1")]), AGENTS).unwrap();
        assert_eq!(cfg.gate.max_attempts, 1);
        // A value that is set is taken as written, and the ceiling bounds the cap.
        let cfg = load(
            &with(&[
                ("ORCH_MAX_ATTEMPTS_CAP", "100"),
                ("ORCH_MAX_ATTEMPTS", "100"),
            ]),
            AGENTS,
        )
        .unwrap();
        assert_eq!(cfg.gate.max_attempts, 100);
    }

    #[test]
    fn both_spellings_of_a_source_are_accepted_in_every_layer() {
        // The API says `agent_checks` (Thread.job.gate); the configuration says `agent-checks`.
        for spelling in ["agent-checks", "agent_checks"] {
            let cfg = load(
                &with(&[("ORCH_GATE", spelling)]),
                &agents_with_gate(&format!("{{require: [{spelling}]}}")),
            )
            .unwrap();
            assert_eq!(
                cfg.gate.require,
                [CheckSource::AgentChecks].into(),
                "{spelling}"
            );
        }
    }

    /// `ORCH_GATE` itself, through `Config` and `AppConfig` to the job of a thread the `App`
    /// creates: the variable is what the job runs under, and a target's entry and a request change
    /// it only the way the rules say.
    #[tokio::test]
    async fn the_gate_variables_reach_the_job_of_a_created_thread() {
        use orch_app::{App, Creation, Inbound, NewThread};
        use orch_core::{AgentTarget, ThreadId};
        use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
        use orch_ports::{PortSet, SystemClock};

        let cfg = load(
            &with(&[("ORCH_GATE", "agent-checks"), ("ORCH_MAX_ATTEMPTS", "2")]),
            &agents_with_gate("{maxAttempts: 4}"),
        )
        .unwrap();
        let directory = AgentDirectory::new(cfg.agents.clone());
        let app = App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: orch_ports::NoModel,
                auth: orch_ports::RefuseAll,
                registry: directory.fixed_registry(),
            },
            directory,
            cfg.app_config(),
        )
        .unwrap();
        let create = |agent: &'static str, gate: Option<GateLayer>| {
            let app = &app;
            async move {
                let inbound = Inbound {
                    gate,
                    ..Inbound::default()
                };
                let new = NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new(agent),
                        release: None,
                    },
                    text: "go".to_owned(),
                };
                match app
                    .create_thread_as(
                        &UserId::new("alice@example.com"),
                        ThreadId(uuid::Uuid::now_v7()),
                        new,
                        inbound,
                    )
                    .await
                {
                    Ok(Creation::Created { thread, .. }) => thread.job.gate,
                    other => panic!("{other:?}"),
                }
            }
        };
        // `plain` has no entry: the variables. `coder` has one: its attempts, the variable's sources.
        let plain = create("plain", None).await;
        assert_eq!(plain.require, [CheckSource::AgentChecks].into());
        assert_eq!(plain.max_attempts, 2);
        let coder = create("coder", None).await;
        assert_eq!(coder.require, [CheckSource::AgentChecks].into());
        assert_eq!(coder.max_attempts, 4);
        // A run may lower them again.
        let asked = GateLayer::from_json(&serde_json::json!({"maxAttempts": 1}))
            .unwrap()
            .unwrap();
        assert_eq!(create("coder", Some(asked)).await.max_attempts, 1);
    }

    // ---- the MCP surface (ADR 0019) ---------------------------------------------------------

    const TOKENS: &str = "\
- user: Alice@Example.com
  tokenEnv: MCP_TOKEN_ALICE
- user: bob@example.com
  tokenEnv: MCP_TOKEN_BOB
";

    /// `load` with a token file: `MCP_TOKENS_FILE` reads `tokens_yaml`, every other path the agents.
    fn load_mcp(pairs: &[(&str, &str)], tokens_yaml: &str) -> Result<Config, ConfigError> {
        let tokens = tokens_yaml.to_owned();
        Config::load(args_of(pairs), env_of(pairs), move |path| {
            if path == Path::new("/etc/orch/mcp-tokens.yaml") {
                Ok(tokens.clone())
            } else if path == Path::new("/etc/orch/agents.yaml") {
                Ok(AGENTS.to_owned())
            } else {
                Err(io::Error::from(io::ErrorKind::NotFound))
            }
        })
    }

    fn mcp_env<'a>() -> Vec<(&'a str, &'a str)> {
        let mut env = base();
        env.extend([
            ("ORCH_SURFACES", "agui,mcp"),
            ("MCP_TOKENS_FILE", "/etc/orch/mcp-tokens.yaml"),
            (
                "MCP_ALLOWED_HOSTS",
                "orch.example.com, orch.example.com:443,",
            ),
            (
                "MCP_TOKEN_ALICE",
                " alice-secret-0123456789abcdef0123456789\n",
            ),
            ("MCP_TOKEN_BOB", "bob-secret-0123456789abcdef012345678901"),
        ]);
        env
    }

    #[cfg(feature = "surface-mcp")]
    #[test]
    fn the_mcp_surface_reads_its_tokens_hosts_and_public_url() {
        let mut env = mcp_env();
        env.push(("ORCH_PUBLIC_URL", "https://chat.example.com/"));
        let cfg = load_mcp(&env, TOKENS).unwrap();
        assert_eq!(cfg.surfaces, vec![Surface::Agui, Surface::Mcp]);
        let mcp = cfg.mcp.unwrap();
        let tokens: Vec<(&str, &str)> = mcp
            .tokens
            .iter()
            .map(|(user, _, token)| (user.as_str(), token.expose_secret()))
            .collect();
        // The user is normalised and the token is trimmed, as everywhere else.
        assert_eq!(
            tokens,
            [
                (
                    "alice@example.com",
                    "alice-secret-0123456789abcdef0123456789"
                ),
                ("bob@example.com", "bob-secret-0123456789abcdef012345678901")
            ]
        );
        assert_eq!(
            mcp.allowed_hosts,
            ["orch.example.com", "orch.example.com:443"]
        );
        assert_eq!(mcp.public_url.as_deref(), Some("https://chat.example.com"));
        assert_eq!(mcp.wait_max, Duration::from_secs(3600), "the default bound");
        // Nothing prints a token.
        let shown = format!("{mcp:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("alice@example.com"), "{shown}");
    }

    #[cfg(feature = "surface-mcp")]
    #[test]
    fn a_token_has_the_role_its_entry_names_and_the_role_must_be_defined() {
        let tokens = "\
- { user: alice@example.com, tokenEnv: MCP_TOKEN_ALICE, role: admin }
- { user: bob@example.com, tokenEnv: MCP_TOKEN_BOB }
";
        let cfg = load_mcp(&mcp_env(), tokens).unwrap();
        let mcp = cfg.mcp.unwrap();
        let roles: Vec<Vec<&str>> = mcp
            .tokens
            .iter()
            .map(|(_, roles, _)| roles.iter().map(orch_ports::Role::as_str).collect())
            .collect();
        assert_eq!(roles, [vec!["admin"], Vec::<&str>::new()]);
        // A role nobody defined is an error naming the file's key, not the default role.
        for (entry, expected) in [
            (
                "role: wizard",
                "the role \"wizard\" of alice@example.com is not one of auth.roles (admin, user)",
            ),
            ("role: ' '", "the role of alice@example.com is empty"),
        ] {
            let tokens =
                format!("- {{ user: alice@example.com, tokenEnv: MCP_TOKEN_ALICE, {entry} }}\n");
            let error = load_mcp(&mcp_env(), &tokens).err().unwrap();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn without_a_file_every_role_the_policy_knows_is_the_built_in_pair() {
        let cfg = load(&base(), AGENTS).unwrap();
        let policy = &cfg.auth.policy;
        assert_eq!(
            policy
                .role_names()
                .map(orch_ports::Role::as_str)
                .collect::<Vec<_>>(),
            ["admin", "user"]
        );
        assert_eq!(
            policy.default_role().map(orch_ports::Role::as_str),
            Some("user")
        );
        assert_eq!(cfg.app_config().policy, *policy);
    }

    #[test]
    fn the_mcp_settings_are_not_read_unless_the_surface_is_mounted() {
        // The default surfaces: no file, no hosts, no error, even for variables that are wrong.
        let mut env = base();
        env.extend([("MCP_TOKENS_FILE", "/nowhere"), ("MCP_ALLOWED_HOSTS", ",")]);
        assert!(load(&env, AGENTS).unwrap().mcp.is_none());
    }

    /// A worker mounts no surface, so it is not asked for the secrets of one.
    #[cfg(all(feature = "surface-agui", feature = "surface-mcp"))]
    #[test]
    fn a_worker_is_not_asked_for_the_mcp_secrets() {
        let mut env = mcp_env();
        env.retain(|(k, _)| !k.starts_with("MCP_TOKEN"));
        env.push(("ORCH_ROLE", "worker"));
        assert!(load_mcp(&env, TOKENS).unwrap().mcp.is_none());
    }

    #[cfg(feature = "surface-mcp")]
    #[test]
    fn the_mcp_surface_fails_closed_on_every_missing_piece() {
        let without = |var: &str| {
            let mut env = mcp_env();
            env.retain(|(k, _)| *k != var);
            load_mcp(&env, TOKENS).unwrap_err()
        };
        assert!(matches!(
            without("MCP_TOKENS_FILE"),
            ConfigError::Missing("MCP_TOKENS_FILE")
        ));
        assert!(matches!(
            without("MCP_ALLOWED_HOSTS"),
            ConfigError::Missing("MCP_ALLOWED_HOSTS")
        ));
        // A token variable that is unset stops the service, and names the variable, not a value.
        let err = without("MCP_TOKEN_BOB");
        assert!(
            matches!(&err, ConfigError::McpTokenEnvMissing { user, var }
            if user == "bob@example.com" && var == "MCP_TOKEN_BOB"),
            "{err}"
        );
        assert!(err.to_string().contains("MCP_TOKEN_BOB"));
        // Empty counts as unset.
        let mut env = mcp_env();
        env.retain(|(k, _)| *k != "MCP_TOKEN_BOB");
        env.push(("MCP_TOKEN_BOB", "  "));
        assert!(matches!(
            load_mcp(&env, TOKENS).unwrap_err(),
            ConfigError::McpTokenEnvMissing { .. }
        ));
        // The file cannot be read.
        let mut env = mcp_env();
        env.retain(|(k, _)| *k != "MCP_TOKENS_FILE");
        env.push(("MCP_TOKENS_FILE", "/etc/orch/elsewhere.yaml"));
        assert!(matches!(
            load_mcp(&env, TOKENS).unwrap_err(),
            ConfigError::McpTokensFileRead { .. }
        ));
    }

    #[cfg(feature = "surface-mcp")]
    #[test]
    fn a_bad_token_file_or_host_list_is_refused_naming_the_variable() {
        let invalid = |env: &[(&str, &str)], yaml: &str, var: &str| {
            let err = load_mcp(env, yaml).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { var: v, .. } if *v == var),
                "{err}"
            );
        };
        let env = mcp_env();
        for yaml in [
            "",
            "[]",
            "user: a@x.io\ntokenEnv: MCP_TOKEN_ALICE\n",
            "- {user: a@x.io}\n",
            "- {user: a@x.io, tokenEnv: MCP_TOKEN_ALICE, scope: all}\n",
            "- {user: not-an-email, tokenEnv: MCP_TOKEN_ALICE}\n",
            "- {user: a@x.io, tokenEnv: ''}\n",
            // One token for two users.
            "- {user: a@x.io, tokenEnv: MCP_TOKEN_ALICE}\n- {user: b@x.io, tokenEnv: MCP_TOKEN_ALICE}\n",
        ] {
            invalid(&env, yaml, "MCP_TOKENS_FILE");
        }
        // Two tokens for one user (a rotation) are fine.
        let rotation = "- {user: a@x.io, tokenEnv: MCP_TOKEN_ALICE}\n- {user: a@x.io, tokenEnv: MCP_TOKEN_BOB}\n";
        assert_eq!(
            load_mcp(&env, rotation).unwrap().mcp.unwrap().tokens.len(),
            2
        );

        for hosts in [",", " , "] {
            let mut env = mcp_env();
            env.retain(|(k, _)| *k != "MCP_ALLOWED_HOSTS");
            env.push(("MCP_ALLOWED_HOSTS", hosts));
            invalid(&env, TOKENS, "MCP_ALLOWED_HOSTS");
        }
        for url in [
            "chat.example.com",
            "ftp://chat.example.com",
            "https://chat.example.com/app",
            "https://chat.example.com/?x=1",
            "https://",
        ] {
            let mut env = mcp_env();
            env.push(("ORCH_PUBLIC_URL", url));
            invalid(&env, TOKENS, "ORCH_PUBLIC_URL");
        }
    }

    #[cfg(feature = "surface-mcp")]
    #[test]
    fn the_wait_bound_is_a_number_of_seconds_within_limits() {
        let with = |value: &'static str| {
            let mut env = mcp_env();
            env.push(("MCP_WAIT_MAX_SECS", value));
            load_mcp(&env, TOKENS)
        };
        assert_eq!(
            with("90").unwrap().mcp.unwrap().wait_max,
            Duration::from_secs(90)
        );
        assert_eq!(
            with(" 1 ").unwrap().mcp.unwrap().wait_max,
            Duration::from_secs(1)
        );
        assert_eq!(
            with("86400").unwrap().mcp.unwrap().wait_max,
            Duration::from_secs(86_400)
        );
        for bad in ["0", "86401", "-5", "soon", "1.5"] {
            let err = with(bad).unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConfigError::Invalid {
                        var: "MCP_WAIT_MAX_SECS",
                        ..
                    }
                ),
                "{bad}: {err}"
            );
        }
    }

    #[cfg(feature = "surface-mcp")]
    #[test]
    fn hosts_must_be_authorities_and_origins_origins() {
        let with = |var: &'static str, value: &'static str| {
            let mut env = mcp_env();
            env.retain(|(k, _)| *k != var);
            env.push((var, value));
            load_mcp(&env, TOKENS)
        };
        // Names, addresses and ports are hosts.
        for good in [
            "orch.example.com",
            "orch.example.com:8443",
            "localhost,127.0.0.1:8080,[::1]:8080",
        ] {
            assert!(with("MCP_ALLOWED_HOSTS", good).is_ok(), "{good}");
        }
        // Anything else would match no Host header, and only look like a rule.
        for bad in [
            "https://orch.example.com",
            "*",
            "*.example.com",
            "orch.example.com/mcp",
            "user@orch.example.com",
            "orch example",
            "orch.example.com:notaport",
        ] {
            let err = with("MCP_ALLOWED_HOSTS", bad).unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConfigError::Invalid {
                        var: "MCP_ALLOWED_HOSTS",
                        ..
                    }
                ),
                "{bad}: {err}"
            );
        }
        assert!(
            with(
                "MCP_ALLOWED_ORIGINS",
                "https://inspector.example.com,http://localhost:6274"
            )
            .is_ok()
        );
        assert!(
            with("MCP_ALLOWED_ORIGINS", "")
                .unwrap()
                .mcp
                .unwrap()
                .allowed_origins
                .is_empty()
        );
        for bad in [
            "inspector.example.com",
            "https://x.example.com/",
            "https://x.example.com/app",
            "*",
            "ftp://x.example.com",
        ] {
            let err = with("MCP_ALLOWED_ORIGINS", bad).unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConfigError::Invalid {
                        var: "MCP_ALLOWED_ORIGINS",
                        ..
                    }
                ),
                "{bad}: {err}"
            );
        }
    }

    #[cfg(feature = "surface-mcp")]
    #[test]
    fn a_token_shorter_than_32_bytes_is_refused() {
        let mut env = mcp_env();
        env.retain(|(k, _)| *k != "MCP_TOKEN_BOB");
        env.push(("MCP_TOKEN_BOB", "0123456789012345678901234567890"));
        let err = load_mcp(&env, TOKENS).unwrap_err();
        let ConfigError::Invalid {
            var: "MCP_TOKENS_FILE",
            reason,
        } = &err
        else {
            panic!("{err}");
        };
        assert!(reason.contains("shorter than 32 bytes"), "{reason}");
        assert!(
            !reason.contains("0123456789012345678901234567890"),
            "the token is not echoed"
        );
        // Exactly 32 is enough.
        env.retain(|(k, _)| *k != "MCP_TOKEN_BOB");
        env.push(("MCP_TOKEN_BOB", "01234567890123456789012345678901"));
        assert!(load_mcp(&env, TOKENS).is_ok());
    }

    #[cfg(feature = "surface-mcp")]
    #[test]
    fn the_wait_limits_default_and_are_at_least_one() {
        let mcp = load_mcp(&mcp_env(), TOKENS).unwrap().mcp.unwrap();
        assert_eq!((mcp.wait_max_concurrent, mcp.wait_max_per_user), (256, 16));
        let mut env = mcp_env();
        env.extend([
            ("MCP_WAIT_MAX_CONCURRENT", "8"),
            ("MCP_WAIT_MAX_PER_USER", "2"),
        ]);
        let mcp = load_mcp(&env, TOKENS).unwrap().mcp.unwrap();
        assert_eq!((mcp.wait_max_concurrent, mcp.wait_max_per_user), (8, 2));
        for var in ["MCP_WAIT_MAX_CONCURRENT", "MCP_WAIT_MAX_PER_USER"] {
            for bad in ["0", "-1", "many"] {
                let mut env = mcp_env();
                env.push((var, bad));
                let err = load_mcp(&env, TOKENS).unwrap_err();
                assert!(
                    matches!(&err, ConfigError::Invalid { var: v, .. } if *v == var),
                    "{var}={bad}: {err}"
                );
            }
        }
    }

    #[cfg(not(feature = "surface-mcp"))]
    #[test]
    fn mcp_is_refused_in_a_build_without_the_feature() {
        let mut env = mcp_env();
        env.retain(|(k, _)| *k != "ORCH_SURFACES");
        env.push(("ORCH_SURFACES", "mcp"));
        let err = load_mcp(&env, TOKENS).unwrap_err();
        assert!(
            matches!(
                &err,
                ConfigError::SurfaceNotCompiled {
                    surface: "mcp",
                    feature: "surface-mcp"
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn the_configuration_never_prints_a_bearer_token() {
        let mut env = mcp_env();
        env.push(("ORCH_PUBLIC_URL", "https://chat.example.com"));
        if let Ok(cfg) = load_mcp(&env, TOKENS) {
            let shown = format!("{cfg:?}");
            assert!(
                !shown.contains("alice-secret") && !shown.contains("bob-secret"),
                "{shown}"
            );
        }
    }

    // ---- the thread-tools endpoint (thread-tools/v1, ADR 0023) ------------------------------

    const TT_SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const TT_PREVIOUS: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn thread_tools_env<'a>() -> Vec<(&'a str, &'a str)> {
        let mut env = base();
        env.extend([
            ("ORCH_SURFACES", "agui,thread-tools"),
            ("THREAD_TOOLS_SECRET", TT_SECRET),
            ("THREAD_TOOLS_URL", "http://orchestrator:8080"),
        ]);
        env
    }

    #[cfg(feature = "surface-thread-tools")]
    #[test]
    fn the_thread_tools_endpoint_reads_its_keys_url_lifetime_and_hosts() {
        let cfg = load(&thread_tools_env(), AGENTS).unwrap();
        assert_eq!(cfg.surfaces, vec![Surface::Agui, Surface::ThreadTools]);
        assert_eq!(Surface::ThreadTools.name(), "thread-tools");
        assert_eq!(Surface::ThreadTools.feature(), "surface-thread-tools");
        let tt = cfg.thread_tools.as_ref().unwrap();
        assert_eq!(tt.issuer.base_url(), "http://orchestrator:8080");
        assert_eq!(tt.issuer.ttl(), Duration::from_secs(7200), "the default");
        // The default host list is the host, and the port, of the URL.
        assert_eq!(tt.allowed_hosts, ["orchestrator:8080"]);
        assert_eq!(tt.issuer.keys().previous_kid(), None);

        // Everything set: a rotation, a lifetime, hosts of its own.
        let mut env = thread_tools_env();
        env.extend([
            ("THREAD_TOOLS_SECRET_PREVIOUS", TT_PREVIOUS),
            ("THREAD_TOOLS_TOKEN_TTL_SECS", "3600"),
            (
                "THREAD_TOOLS_ALLOWED_HOSTS",
                "orchestrator:8080, 127.0.0.1:8080,localhost,",
            ),
        ]);
        let cfg = load(&env, AGENTS).unwrap();
        let tt = cfg.thread_tools.as_ref().unwrap();
        assert_eq!(tt.issuer.ttl(), Duration::from_secs(3600));
        assert!(tt.issuer.keys().previous_kid().is_some());
        assert_eq!(
            tt.allowed_hosts,
            ["orchestrator:8080", "127.0.0.1:8080", "localhost"]
        );

        // A URL without a port: the host alone; a name without a port matches any port.
        let mut env = thread_tools_env();
        env.retain(|(k, _)| *k != "THREAD_TOOLS_URL");
        env.push(("THREAD_TOOLS_URL", "https://orch.example.com/"));
        let cfg = load(&env, AGENTS).unwrap();
        let tt = cfg.thread_tools.as_ref().unwrap();
        assert_eq!(tt.issuer.base_url(), "https://orch.example.com");
        assert_eq!(tt.allowed_hosts, ["orch.example.com"]);
    }

    #[cfg(feature = "surface-thread-tools")]
    #[test]
    fn the_keys_and_url_are_read_by_every_role_and_without_the_surface() {
        // Workers mint, the control plane serves: a worker is given the same variables, and
        // needs them when the surface is named.
        let mut env = thread_tools_env();
        env.push(("ORCH_ROLE", "worker"));
        assert!(load(&env, AGENTS).unwrap().thread_tools.is_some());
        env.retain(|(k, _)| !k.starts_with("THREAD_TOOLS"));
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::MissingForSurface {
                var: "THREAD_TOOLS_SECRET",
                surface: "thread-tools"
            }
        ));
        // The surface is not named here (the control plane elsewhere serves it): the adapter
        // still mints, so the keys and the URL are read.
        let mut env = thread_tools_env();
        env.retain(|(k, _)| *k != "ORCH_SURFACES");
        assert!(load(&env, AGENTS).unwrap().thread_tools.is_some());
        // And neither being set is no thread tools, and not an error.
        let cfg = load(&base(), AGENTS).unwrap();
        assert!(cfg.thread_tools.is_none());
    }

    #[cfg(feature = "surface-thread-tools")]
    #[test]
    fn the_thread_tools_endpoint_fails_closed_on_every_missing_or_bad_piece() {
        type Env = Vec<(&'static str, &'static str)>;
        let err = |change: &dyn Fn(&mut Env)| {
            let mut env = thread_tools_env();
            change(&mut env);
            load(&env, AGENTS).unwrap_err().to_string()
        };
        let without = |var: &'static str| move |env: &mut Env| env.retain(|(k, _)| *k != var);
        let setting = |var: &'static str, value: &'static str| {
            move |env: &mut Env| {
                env.retain(|(k, _)| *k != var);
                env.push((var, value));
            }
        };
        // the surface is named and the secret is not (the URL alone is the other mistake)
        assert!(
            err(&|env| {
                without("THREAD_TOOLS_SECRET")(env);
                without("THREAD_TOOLS_URL")(env);
            })
            .contains("THREAD_TOOLS_SECRET is required when ORCH_SURFACES mounts \"thread-tools\"")
        );
        assert!(
            err(&without("THREAD_TOOLS_SECRET"))
                .contains("THREAD_TOOLS_URL is invalid: THREAD_TOOLS_SECRET is not set")
        );
        assert!(
            err(&without("THREAD_TOOLS_URL"))
                .contains("THREAD_TOOLS_SECRET is invalid: THREAD_TOOLS_URL is not set")
        );
        // a key that is too short, and a previous one that is
        assert!(
            err(&setting("THREAD_TOOLS_SECRET", "too-short"))
                .contains("THREAD_TOOLS_SECRET is invalid: the current key is shorter than 32")
        );
        assert!(
            err(&setting("THREAD_TOOLS_SECRET_PREVIOUS", "too-short"))
                .contains("THREAD_TOOLS_SECRET_PREVIOUS is invalid")
        );
        // a previous key that is the current one, and one with no current key
        assert!(
            err(&setting("THREAD_TOOLS_SECRET_PREVIOUS", TT_SECRET))
                .contains("THREAD_TOOLS_SECRET_PREVIOUS is invalid: the previous key is the same")
        );
        let mut only_previous = base();
        only_previous.push(("THREAD_TOOLS_SECRET_PREVIOUS", TT_PREVIOUS));
        assert!(
            load(&only_previous, AGENTS)
                .unwrap_err()
                .to_string()
                .contains("THREAD_TOOLS_SECRET_PREVIOUS is invalid")
        );
        // a URL that is not an http(s) URL with a host
        for bad in [
            "orchestrator:8080",
            "ftp://orchestrator",
            "http://u:p@orchestrator",
            "http://orchestrator?x=1",
        ] {
            assert!(
                err(&setting("THREAD_TOOLS_URL", bad)).contains("THREAD_TOOLS_URL is invalid"),
                "{bad}"
            );
        }
        // a lifetime outside 60 to 86400
        for bad in ["59", "86401", "0", "soon", "-1"] {
            assert!(
                err(&setting("THREAD_TOOLS_TOKEN_TTL_SECS", bad))
                    .contains("THREAD_TOOLS_TOKEN_TTL_SECS is invalid"),
                "{bad}"
            );
        }
        // hosts that are not Host values
        for bad in [
            "*",
            "https://orchestrator:8080",
            "orchestrator/",
            ",",
            "orch:port",
        ] {
            assert!(
                err(&setting("THREAD_TOOLS_ALLOWED_HOSTS", bad))
                    .contains("THREAD_TOOLS_ALLOWED_HOSTS is invalid"),
                "{bad}"
            );
        }
        // The boundaries pass.
        for ok in ["60", "86400"] {
            let mut env = thread_tools_env();
            env.push(("THREAD_TOOLS_TOKEN_TTL_SECS", ok));
            assert!(load(&env, AGENTS).is_ok(), "{ok}");
        }
    }

    #[cfg(feature = "surface-thread-tools")]
    #[test]
    fn the_configuration_never_prints_a_thread_tools_key() {
        let mut env = thread_tools_env();
        env.push(("THREAD_TOOLS_SECRET_PREVIOUS", TT_PREVIOUS));
        let cfg = load(&env, AGENTS).unwrap();
        let shown = format!("{cfg:?}");
        assert!(shown.contains("thread_tools"), "{shown}");
        assert!(!shown.contains(TT_SECRET), "{shown}");
        assert!(!shown.contains(TT_PREVIOUS), "{shown}");
        // Nor does an error: a key that is refused is not echoed.
        let mut env = thread_tools_env();
        env.retain(|(k, _)| *k != "THREAD_TOOLS_URL");
        let shown = load(&env, AGENTS).unwrap_err().to_string();
        assert!(!shown.contains(TT_SECRET), "{shown}");
    }

    #[cfg(not(feature = "surface-thread-tools"))]
    #[test]
    fn thread_tools_is_refused_in_a_build_without_the_feature() {
        let mut env = thread_tools_env();
        env.retain(|(k, _)| *k != "ORCH_SURFACES");
        env.push(("ORCH_SURFACES", "thread-tools"));
        let err = load(&env, AGENTS).unwrap_err();
        assert!(
            matches!(
                &err,
                ConfigError::SurfaceNotCompiled {
                    surface: "thread-tools",
                    feature: "surface-thread-tools"
                }
            ),
            "{err}"
        );
        // The variables alone are not read in such a build.
        let mut env = base();
        env.extend([("THREAD_TOOLS_SECRET", "x")]);
        assert!(load(&env, AGENTS).is_ok());
    }
}
