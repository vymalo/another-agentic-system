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

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use adam_host::Role;
use clap::Parser;
use orch_app::AgentEntry;
use orch_core::{AgentId, UserId};
use orch_ports::AgentEndpoint;
use serde::Deserialize;
use url::Url;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_DATABASE_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_DISPATCHER_CONCURRENCY: usize = 32;
const DEFAULT_OUTBOX_LEASE_SECS: u64 = 30;
#[cfg(feature = "agent-local")]
const DEFAULT_AGENT_LOCAL_CONCURRENCY: usize = 4;
const DEFAULT_SHUTDOWN_GRACE_SECS: u64 = 15;
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
    pub const ALL: &'static [Surface] = &[Surface::Agui];

    /// The name used in `ORCH_SURFACES`.
    pub const fn name(self) -> &'static str {
        match self {
            Surface::Agui => "agui",
        }
    }

    /// The Cargo feature of the `orchestrator` package that compiles the surface in.
    pub const fn feature(self) -> &'static str {
        match self {
            Surface::Agui => "surface-agui",
        }
    }

    /// Whether this build contains the surface.
    pub const fn compiled_in(self) -> bool {
        match self {
            Surface::Agui => cfg!(feature = "surface-agui"),
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

    /// YAML list of `{id, name, transport?, cardUrl?, tokenEnv?, agent?}` (required).
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
    /// it). Known: agui. The removed legacy `chat-api` is refused at startup. The resource API
    /// and health are always mounted.
    #[arg(long, env = "ORCH_SURFACES", value_name = "LIST")]
    pub surfaces: Option<String>,

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
    /// `ORCH_ROLE`: which halves this process runs (`adam-host`'s closed enum; default `all`).
    pub role: Role,
    /// `ORCH_SURFACES`: the interaction surfaces to mount, no repeats. Empty only when the
    /// variable is unset in a build that contains no default surface.
    pub surfaces: Vec<Surface>,
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
            .field("role", &self.role)
            .field("surfaces", &self.surfaces)
            .field("auth_dev_user", &self.auth_dev_user)
            .field("database_max_connections", &self.database_max_connections)
            .field("dispatcher_concurrency", &self.dispatcher_concurrency)
            .field("outbox_lease", &self.outbox_lease)
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
        let agents = parse_agents(&text, &agents_file, get_env)?;

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
            role,
            surfaces,
            auth_dev_user,
            database_max_connections,
            dispatcher_concurrency,
            #[cfg(feature = "agent-local")]
            agent_local_concurrency,
            outbox_lease: Duration::from_secs(outbox_lease_secs),
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
fn parse_agents(
    yaml: &str,
    path: &Path,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<AgentEntry>, ConfigError> {
    parse_agents_with(yaml, path, env, LocalAgentKind::compiled_in)
}

/// [`parse_agents`] with the build's set of local agent kinds as a parameter, so the tests can
/// exercise a build that has them.
fn parse_agents_with(
    yaml: &str,
    path: &Path,
    env: impl Fn(&str) -> Option<String>,
    local_compiled_in: impl Fn(LocalAgentKind) -> bool,
) -> Result<Vec<AgentEntry>, ConfigError> {
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
        entries.push(AgentEntry {
            endpoint,
            name: spec.name.trim().to_owned(),
        });
    }
    Ok(entries)
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
                "AUTH_DEV_USER" => &mut args.auth_dev_user,
                "DATABASE_MAX_CONNECTIONS" => &mut args.database_max_connections,
                "DISPATCHER_CONCURRENCY" => &mut args.dispatcher_concurrency,
                #[cfg(feature = "agent-local")]
                "AGENT_LOCAL_CONCURRENCY" => &mut args.agent_local_concurrency,
                "OUTBOX_LEASE_SECS" => &mut args.outbox_lease_secs,
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
            "AUTH_DEV_USER",
            "DATABASE_MAX_CONNECTIONS",
            "DISPATCHER_CONCURRENCY",
            "OUTBOX_LEASE_SECS",
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
}
