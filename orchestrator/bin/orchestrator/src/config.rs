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

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use adam_host::Role;
use clap::Parser;
use orch_app::{
    AgentDirectory, AgentEntry, AppConfig, DEFAULT_MAX_ATTEMPTS_CAP, GateLayer, GateRules,
    InboxConfig, Layer, MAX_ATTEMPTS_CAP_CEILING, known_sources,
};
use orch_core::{AgentId, CheckSource, DEFAULT_MAX_ATTEMPTS, GatePolicy, UserId};
use orch_ports::AgentEndpoint;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use url::Url;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_DATABASE_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_DISPATCHER_CONCURRENCY: usize = 32;
const DEFAULT_OUTBOX_LEASE_SECS: u64 = 30;
#[cfg(feature = "agent-local")]
const DEFAULT_AGENT_LOCAL_CONCURRENCY: usize = 4;
const DEFAULT_SHUTDOWN_GRACE_SECS: u64 = 15;
const DEFAULT_MCP_WAIT_MAX_SECS: u64 = 3600;
/// The largest `MCP_WAIT_MAX_SECS`: a day. A wait is a request that stays open.
const MAX_MCP_WAIT_MAX_SECS: u64 = 86_400;
const MAX_AGENT_ID_LEN: usize = 63;

/// A configuration problem. The message is what the operator sees.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required environment variable is unset or empty.
    #[error("{0} is required")]
    Missing(&'static str),
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
    /// `MCP_TOKENS_FILE`, resolved: each `tokenEnv` already read. A user may have several tokens.
    pub tokens: Vec<(UserId, SecretString)>,
    /// `MCP_ALLOWED_HOSTS`: the `Host` values the server accepts.
    pub allowed_hosts: Vec<String>,
    /// `ORCH_PUBLIC_URL`: the chat's public origin, for the `web_url` of `start_job`.
    pub public_url: Option<String>,
    /// `MCP_WAIT_MAX_SECS`: the largest `timeout_secs` of `wait_for_job`.
    pub wait_max: Duration,
}

impl fmt::Debug for McpSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpSettings")
            .field(
                "tokens",
                &self.tokens.iter().map(|(user, _)| user).collect::<Vec<_>>(),
            )
            .field("allowed_hosts", &self.allowed_hosts)
            .field("public_url", &self.public_url)
            .field("wait_max", &self.wait_max)
            .finish()
    }
}

/// One entry of `MCP_TOKENS_FILE`, as written.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TokenSpec {
    user: String,
    token_env: String,
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
    pub const ALL: &'static [Surface] = &[Surface::Agui, Surface::Mcp];

    /// The name used in `ORCH_SURFACES`.
    pub const fn name(self) -> &'static str {
        match self {
            Surface::Agui => "agui",
            Surface::Mcp => "mcp",
        }
    }

    /// The Cargo feature of the `orchestrator` package that compiles the surface in.
    pub const fn feature(self) -> &'static str {
        match self {
            Surface::Agui => "surface-agui",
            Surface::Mcp => "surface-mcp",
        }
    }

    /// Whether this build contains the surface.
    pub const fn compiled_in(self) -> bool {
        match self {
            Surface::Agui => cfg!(feature = "surface-agui"),
            Surface::Mcp => cfg!(feature = "surface-mcp"),
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
[env: NAME]. A flag wins over its variable. An empty value counts as unset. The bearer token \
of an agent is read from the variable its `tokenEnv` names in AGENTS_FILE, never from a flag. \
Logging is filtered by RUST_LOG (default: info)."
)]
pub struct Args {
    /// Postgres connection string (required). Never logged.
    #[arg(long, env = "DATABASE_URL", value_name = "URL", hide_env_values = true)]
    pub database_url: Option<String>,

    /// YAML list of `{id, name, transport?, cardUrl?, tokenEnv?, agent?, gate?}` (required).
    #[arg(long, env = "AGENTS_FILE", value_name = "PATH")]
    pub agents_file: Option<String>,

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
    /// MCP_ALLOWED_HOSTS). The removed legacy `chat-api` is refused at startup. The resource API
    /// and health are always mounted.
    #[arg(long, env = "ORCH_SURFACES", value_name = "LIST")]
    pub surfaces: Option<String>,

    /// Sources every job's work must pass before it is done, comma separated: ci, agent-checks,
    /// verifier (default none: an agent that completes is done). This build honours only
    /// agent-checks; ci and verifier are refused until their slices land.
    #[arg(long, env = "ORCH_GATE", value_name = "LIST")]
    pub gate: Option<String>,

    /// Attempts a job's agent gets under a gate, the first included, at least 1 (default 3).
    #[arg(long, env = "ORCH_MAX_ATTEMPTS", value_name = "N")]
    pub max_attempts: Option<String>,

    /// The most a target or a thread may raise the attempts to, at least 1 (default 10).
    #[arg(long, env = "ORCH_MAX_ATTEMPTS_CAP", value_name = "N")]
    pub max_attempts_cap: Option<String>,

    /// The agent that verifies (an id in AGENTS_FILE). Refused until the verifier dispatch is
    /// built.
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

/// The complete, validated configuration.
pub struct Config {
    /// `DATABASE_URL`. May contain a password: never logged.
    pub database_url: String,
    /// `LISTEN_ADDR`.
    pub listen_addr: SocketAddr,
    /// `AGENTS_FILE`, resolved: bearer tokens already read from their environment variables.
    pub agents: Vec<AgentEntry>,
    /// The gate new threads start under before an agent's entry or a request changes it:
    /// `ORCH_GATE`, `ORCH_MAX_ATTEMPTS`, `ORCH_VERIFIER`.
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
    /// `AUTH_DEV_USER`: an identity for requests without `X-Auth-Request-Email`. Dev only.
    pub auth_dev_user: Option<UserId>,
    /// `DATABASE_MAX_CONNECTIONS` (at least 2: the wakeup listener holds one).
    pub database_max_connections: u32,
    /// `DISPATCHER_CONCURRENCY`: outbox rows processed at once.
    pub dispatcher_concurrency: usize,
    /// `AGENT_LOCAL_CONCURRENCY`: runs of local agents stepped at once.
    #[cfg(feature = "agent-local")]
    pub agent_local_concurrency: usize,
    /// `OUTBOX_LEASE_SECS`: how long a crashed replica's claim blocks others.
    pub outbox_lease: Duration,
    /// `INBOX_LEASE_SECS`, `INBOX_POLL_SECS`, `INBOX_PARKED_TTL_SECS`, `INBOX_MAX_ATTEMPTS`: the
    /// inbox worker (timers and reports).
    pub inbox: InboxConfig,
    /// `ORCH_INSTANCE_ID`: names this replica in outbox leases.
    pub instance_id: String,
    /// `SHUTDOWN_GRACE_SECS`: how long a graceful shutdown may take.
    pub shutdown_grace: Duration,
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
            .field("database_max_connections", &self.database_max_connections)
            .field("dispatcher_concurrency", &self.dispatcher_concurrency)
            .field("outbox_lease", &self.outbox_lease)
            .field("inbox", &self.inbox)
            .field("instance_id", &self.instance_id)
            .field("shutdown_grace", &self.shutdown_grace);
        #[cfg(feature = "agent-local")]
        debug.field("agent_local_concurrency", &self.agent_local_concurrency);
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
        let clean = |v: Option<String>| v.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let get_env = |name: &str| clean(env(name));

        let database_url = clean(args.database_url).ok_or(ConfigError::Missing("DATABASE_URL"))?;
        let listen_addr = clean(args.listen_addr).unwrap_or_else(|| DEFAULT_LISTEN_ADDR.to_owned());
        let listen_addr = listen_addr
            .parse::<SocketAddr>()
            .map_err(|e| ConfigError::Invalid {
                var: "LISTEN_ADDR",
                reason: format!("{listen_addr:?} is not a socket address like 0.0.0.0:8080 ({e})"),
            })?;

        let agents_file =
            PathBuf::from(clean(args.agents_file).ok_or(ConfigError::Missing("AGENTS_FILE"))?);
        let text = read(&agents_file).map_err(|source| ConfigError::AgentsFileRead {
            path: agents_file.clone(),
            source,
        })?;
        let (agents, target_gates) =
            parse_agents_full(&text, &agents_file, get_env, LocalAgentKind::compiled_in)?;
        let (gate, gate_rules) = parse_gate(
            GateVars {
                gate: clean(args.gate),
                max_attempts: clean(args.max_attempts),
                max_attempts_cap: clean(args.max_attempts_cap),
                verifier: clean(args.verifier),
            },
            &agents,
            &target_gates,
        )?;

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
            })
        } else {
            None
        };

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
            gate,
            gate_rules,
            target_gates,
            role,
            surfaces,
            mcp,
            auth_dev_user,
            database_max_connections,
            dispatcher_concurrency,
            #[cfg(feature = "agent-local")]
            agent_local_concurrency,
            outbox_lease: Duration::from_secs(outbox_lease_secs),
            inbox,
            instance_id,
            shutdown_grace: Duration::from_secs(shutdown_grace_secs),
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
        AppConfig {
            gate: self.gate.clone(),
            target_gates: self.target_gates.clone(),
            gate_rules: self.gate_rules.clone(),
            ..AppConfig::default()
        }
    }
}

/// The raw values of the gate's environment variables (blank already counted as unset).
struct GateVars {
    gate: Option<String>,
    max_attempts: Option<String>,
    max_attempts_cap: Option<String>,
    verifier: Option<String>,
}

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
    let rules = GateRules::new(cap);
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

    // Each target's entry on top of the deployment, then every agent's verifier against the
    // configured agents. Both name the agent at fault.
    let refused = |e: orch_app::GateError| ConfigError::Gate {
        context: "AGENTS_FILE",
        reason: e.to_string(),
    };
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
    Ok(hosts)
}

/// `MCP_TOKENS_FILE`: the YAML list, with each token read from the variable its `tokenEnv` names.
/// A user may appear more than once (a rotation); a token may belong to one user only.
fn parse_mcp_tokens(
    path: Option<String>,
    env: &impl Fn(&str) -> Option<String>,
    read: &impl Fn(&Path) -> io::Result<String>,
) -> Result<Vec<(UserId, SecretString)>, ConfigError> {
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
            "{} is not a list of {{user, tokenEnv}}: {e}",
            path.display()
        ))
    })?;
    let specs = specs.unwrap_or_default();
    if specs.is_empty() {
        return Err(invalid(format!("{} lists no tokens", path.display())));
    }
    let mut tokens: Vec<(UserId, SecretString)> = Vec::with_capacity(specs.len());
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
        let user = UserId::new(user);
        if let Some((other, _)) = tokens
            .iter()
            .find(|(_, t)| t.expose_secret() == token.as_str())
        {
            return Err(invalid(format!(
                "the same token is configured for {other} and for {user}"
            )));
        }
        tokens.push((user, SecretString::from(token)));
    }
    Ok(tokens)
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

/// `^[a-z0-9][a-z0-9-]{0,62}$`: safe in URLs, file names and labels.
fn valid_agent_id(id: &str) -> bool {
    let mut chars = id.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first_ok
        && id.len() <= MAX_AGENT_ID_LEN
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
    parse_agents_full(yaml, path, env, local_compiled_in).map(|(entries, _)| entries)
}

/// The entries of the YAML list and the `gate` key of each that has one.
fn parse_agents_full(
    yaml: &str,
    path: &Path,
    env: impl Fn(&str) -> Option<String>,
    local_compiled_in: impl Fn(LocalAgentKind) -> bool,
) -> Result<(Vec<AgentEntry>, BTreeMap<AgentId, GateLayer>), ConfigError> {
    let specs: Option<Vec<AgentSpec>> =
        serde_norway::from_str(yaml).map_err(|e| ConfigError::AgentsFileParse {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
    let specs = specs.unwrap_or_default();
    if specs.is_empty() {
        return Err(ConfigError::NoAgents {
            path: path.to_owned(),
        });
    }

    let mut entries: Vec<AgentEntry> = Vec::with_capacity(specs.len());
    let mut gates: BTreeMap<AgentId, GateLayer> = BTreeMap::new();
    for spec in specs {
        if !valid_agent_id(&spec.id) {
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

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    /// The `Args` the process environment `pairs` would produce, built by hand so no test
    /// depends on (or changes) the real environment. Variables that are not settings of the
    /// service (the `tokenEnv` ones) stay in `pairs` and are read through `env_of`.
    fn args_of(pairs: &[(&str, &str)]) -> Args {
        let mut args = Args::default();
        for (name, value) in pairs {
            let slot = match *name {
                "DATABASE_URL" => &mut args.database_url,
                "AGENTS_FILE" => &mut args.agents_file,
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
        assert_eq!(cfg.inbox.lease, Duration::from_secs(30));
        assert_eq!(cfg.inbox.poll_interval, Duration::from_secs(2));
        assert_eq!(cfg.inbox.parked_ttl, Duration::from_secs(86_400));
        assert_eq!(cfg.inbox.max_attempts, 10);
        assert_eq!(cfg.shutdown_grace, Duration::from_secs(15));
        assert!(cfg.auth_dev_user.is_none());
        assert!(cfg.instance_id.starts_with("orchestrator-"));
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
        assert!(valid_agent_id("a"));
        assert!(valid_agent_id("coder-2"));
        assert!(valid_agent_id(&"a".repeat(63)));
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
    fn ci_and_the_verifier_are_refused_at_startup_naming_the_slice() {
        for (pairs, var, slice) in [
            (vec![("ORCH_GATE", "ci")], "ORCH_GATE", "slice 5"),
            (
                vec![("ORCH_GATE", "agent-checks,ci")],
                "ORCH_GATE",
                "slice 6",
            ),
            (vec![("ORCH_GATE", "verifier")], "ORCH_GATE", "slice 10"),
            (
                vec![("ORCH_VERIFIER", "plain")],
                "ORCH_VERIFIER",
                "slice 10",
            ),
        ] {
            let err = load(&with(&pairs), AGENTS).unwrap_err();
            let (got, reason) = invalid_var(err);
            assert_eq!(got, var);
            assert!(reason.contains(slice), "{pairs:?}: {reason}");
            assert!(
                reason.contains("only agent-checks can be required"),
                "{reason}"
            );
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

    #[test]
    fn a_target_gate_that_this_build_cannot_honour_is_refused_naming_the_agent_and_the_slice() {
        for (gate, slice) in [
            ("{require: [ci]}", "slice 5"),
            ("{require: [agent-checks, ci]}", "slice 6"),
            ("{require: [verifier], verifier: plain}", "slice 10"),
            ("{verifier: plain}", "slice 10"),
            ("{ci: {required: [build], timeoutSecs: 60}}", "slice 5"),
        ] {
            let err = load(&base(), &agents_with_gate(gate)).unwrap_err();
            let ConfigError::Gate { context, reason } = &err else {
                panic!("{gate}: expected a gate error, got {err}");
            };
            assert_eq!(*context, "AGENTS_FILE");
            assert!(reason.contains("coder"), "{gate}: {reason}");
            assert!(reason.contains(slice), "{gate}: {reason}");
            assert!(err.to_string().starts_with("AGENTS_FILE: "), "{err}");
        }
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
        let app = App::new(
            PortSet {
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
            },
            AgentDirectory::new(cfg.agents.clone()),
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
            ("MCP_TOKEN_ALICE", " alice-secret\n"),
            ("MCP_TOKEN_BOB", "bob-secret"),
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
            .map(|(user, token)| (user.as_str(), token.expose_secret()))
            .collect();
        // The user is normalised and the token is trimmed, as everywhere else.
        assert_eq!(
            tokens,
            [
                ("alice@example.com", "alice-secret"),
                ("bob@example.com", "bob-secret")
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
}
