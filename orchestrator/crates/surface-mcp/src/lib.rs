//! The MCP server surface (ADR 0019): the tools `list_agents`, `start_job`, `get_job`,
//! `wait_for_job`, `answer` and `cancel_job` over MCP streamable HTTP, at `/mcp`, so that Claude Code, opencode or any MCP
//! client can give the orchestrator a job and follow it.
//!
//! # Where it sits
//!
//! An adapter over [`orch_app::App`], like every inbound surface (ADR 0009). It calls `App`
//! directly (`create_thread_as`, `submit`, `cancel`): the caller is authenticated and waits for
//! the answer, so nothing goes through the inbox (ADR 0019). It is one of the surfaces the binary
//! mounts with `ORCH_SURFACES` (name `mcp`, Cargo feature `surface-mcp`).
//!
//! # A machine route
//!
//! The route is [outside the identity layer](orch_api::SurfaceRoutes::machine): an MCP client has
//! no oauth2-proxy cookie. The surface guards it itself with **static bearer tokens**
//! ([`TokenTable`]): no `Authorization: Bearer <token>`, or a token nobody has, is `401` with
//! `WWW-Authenticate: Bearer` before any tool runs, and nothing is written. The token maps to a
//! user, which owns the jobs the caller starts and sees, exactly the scope an edge identity has.
//! `X-Auth-Request-Email` is never read.
//!
//! The server validates the `Host` header (DNS rebinding): [`McpConfig`] requires the list of
//! hosts it accepts, so a deployment behind a public name has to name it.
//!
//! # Stateless
//!
//! `rmcp`'s `StreamableHttpService` runs with `legacy_session_mode = false` and no session
//! manager state: a fresh server is built for each request and nothing lives in the process, so
//! any replica serves any request. A job is the thread (ADR 0016); `job_id` is its id.
//!
//! ```text
//! start_job {text, agent?, title?, client_request_id?} -> {job_id, state, created, web_url?}
//! get_job {job_id}        -> state, attempt, branch, pull request, last CI result, findings
//! wait_for_job {job_id, after_seq?, timeout_secs}
//!                         -> follows the job (progress notifications), then the summary + outcome
//! answer {job_id, text}   -> {job_id, state}
//! cancel_job {job_id}     -> {job_id, state, finished}
//! list_agents {}          -> {agents: [{id, name, description}], unavailable_sources?: [name]}
//! ```
//!
//! # Response framing
//!
//! A call is answered with one `application/json` response (rmcp's `json_response`), sent when the
//! tool has answered, with the answer in the same write as the head. Only a call whose tool speaks
//! before it answers, `wait_for_job` with a `progressToken`, gets a `text/event-stream`, and its
//! head goes out with the first notification. The head is never sent before the tool has said
//! anything: a Go reverse proxy (Caddy; oauth2-proxy is unverified) can drop the upstream
//! connection as soon as it has written the head to its client, when it has not yet finished
//! reading the request body it forwarded, and then everything the server writes after the head is
//! lost. With an event stream opened before the answer, that was the whole answer
//! (`tests/framing.rs`).

mod auth;
mod id;
mod job;
mod server;
mod slots;
mod tools;
pub mod wait;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::middleware::from_fn_with_state;
use orch_api::SurfaceRoutes;
use orch_app::App;
use orch_core::ThreadId;
use orch_ports::Ports;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

pub use auth::{MIN_TOKEN_BYTES, McpUser, TokenError, TokenTable};
// Moved to `orch-api`, which every surface shares; kept here under its old name.
pub use id::{MAX_CLIENT_REQUEST_ID_BYTES, job_id_for};
pub use job::{Branch, Findings, JobSummary, LastCheck, PullRequest};
pub use orch_api::is_host_authority;
pub use slots::{Busy, WaitPermit, WaitSlots};
pub use tools::{
    AnswerArgs, CancelJobArgs, GateArgs, GetJobArgs, NoArgs, StartJobArgs, ToolName, WaitForJobArgs,
};

/// The path the server is mounted at.
pub const MCP_PATH: &str = "/mcp";

/// Why an [`McpConfig`] cannot be built.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// No host is allowed: every request would be refused. `rmcp` would fall back to loopback
    /// only; the surface asks for the list instead of choosing one.
    #[error("no allowed host is configured")]
    NoAllowedHosts,
    /// A host entry with no characters.
    #[error("an allowed host is empty")]
    EmptyHost,
    /// A host entry that is not a host name, an address, or either with a port: `https://…`,
    /// `*`, a path or credentials never match the `Host` header, so they would refuse everyone
    /// (or, worse, look like a rule that is not one).
    #[error("the allowed host {0:?} is not a host name or address, with or without a port")]
    BadHost(String),
    /// An allowed origin that is not `http(s)://host[:port]`.
    #[error("the allowed origin {0:?} is not of the form https://host[:port]")]
    BadOrigin(String),
    /// The public URL is not an `http` or `https` URL.
    #[error("the public URL {0:?} is not an http(s) URL")]
    BadPublicUrl(String),
    /// A wait bound, heartbeat, limit or timeout of zero: `wait_for_job` could not wait, or would never stop
    /// sending heartbeats.
    #[error("{0} must not be zero")]
    Zero(&'static str),
}

/// The default largest `timeout_secs` of `wait_for_job` (`MCP_WAIT_MAX_SECS`).
pub const DEFAULT_WAIT_MAX: Duration = Duration::from_secs(3600);
/// The default interval of the heartbeat notification of `wait_for_job`.
pub const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(60);
/// The default most `wait_for_job` calls one process holds open (`MCP_WAIT_MAX_CONCURRENT`).
pub const DEFAULT_WAIT_MAX_CONCURRENT: usize = 256;
/// The default most `wait_for_job` calls one user holds open (`MCP_WAIT_MAX_PER_USER`).
pub const DEFAULT_WAIT_MAX_PER_USER: usize = 16;
/// How long any other tool may take before it is cut off. A machine route has no request
/// timeout of its own (only `wait_for_job` is meant to stay open).
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether `origin` is an `Origin` value: `http(s)://host[:port]` and nothing after it.
pub fn is_origin(origin: &str) -> bool {
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    matches!(uri.scheme_str(), Some("http" | "https"))
        && uri.authority().is_some_and(|a| !a.as_str().contains('@'))
        && uri.query().is_none()
        && (uri.path() == "/" || uri.path().is_empty())
        && !origin.ends_with('/')
}

/// What the surface needs to run.
#[derive(Debug)]
pub struct McpConfig {
    tokens: TokenTable,
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
    public_url: Option<String>,
    wait_max: Duration,
    heartbeat: Duration,
    wait_max_concurrent: usize,
    wait_max_per_user: usize,
    tool_timeout: Duration,
}

impl McpConfig {
    /// A configuration with `tokens` and the `Host` values the server accepts (`example.com`,
    /// `example.com:8080`, `localhost`).
    pub fn new(
        tokens: TokenTable,
        allowed_hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ConfigError> {
        let allowed_hosts: Vec<String> = allowed_hosts
            .into_iter()
            .map(|h| h.into().trim().to_owned())
            .collect();
        if allowed_hosts.is_empty() {
            return Err(ConfigError::NoAllowedHosts);
        }
        if allowed_hosts.iter().any(String::is_empty) {
            return Err(ConfigError::EmptyHost);
        }
        if let Some(bad) = allowed_hosts.iter().find(|h| !is_host_authority(h)) {
            return Err(ConfigError::BadHost(bad.clone()));
        }
        Ok(McpConfig {
            tokens,
            allowed_hosts,
            allowed_origins: Vec::new(),
            public_url: None,
            wait_max: DEFAULT_WAIT_MAX,
            heartbeat: DEFAULT_HEARTBEAT,
            wait_max_concurrent: DEFAULT_WAIT_MAX_CONCURRENT,
            wait_max_per_user: DEFAULT_WAIT_MAX_PER_USER,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
        })
    }

    /// Browser origins that may call the server. Origin validation is always on (MCP asks for
    /// it): a request that carries an `Origin` header is refused (403) unless it is listed here,
    /// and a request without one (every non-browser client) is not affected. Default: none.
    pub fn with_allowed_origins(
        mut self,
        origins: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ConfigError> {
        let origins: Vec<String> = origins
            .into_iter()
            .map(|o| o.into().trim().to_owned())
            .collect();
        if let Some(bad) = origins.iter().find(|o| !is_origin(o)) {
            return Err(ConfigError::BadOrigin(bad.clone()));
        }
        self.allowed_origins = origins;
        Ok(self)
    }

    /// How many `wait_for_job` calls the process holds open at once (default
    /// [`DEFAULT_WAIT_MAX_CONCURRENT`]), and how many one user may (default
    /// [`DEFAULT_WAIT_MAX_PER_USER`]); a call over either limit is a tool error, "too many
    /// waits". Each at least 1.
    pub fn with_wait_limits(
        mut self,
        per_process: usize,
        per_user: usize,
    ) -> Result<Self, ConfigError> {
        if per_process == 0 || per_user == 0 {
            return Err(ConfigError::Zero("a wait limit"));
        }
        self.wait_max_concurrent = per_process;
        self.wait_max_per_user = per_user;
        Ok(self)
    }

    /// How long any tool but `wait_for_job` may take (default [`DEFAULT_TOOL_TIMEOUT`]).
    pub fn with_tool_timeout(mut self, timeout: Duration) -> Result<Self, ConfigError> {
        if timeout.is_zero() {
            return Err(ConfigError::Zero("the tool timeout"));
        }
        self.tool_timeout = timeout;
        Ok(self)
    }

    /// The largest `timeout_secs` `wait_for_job` honours (`MCP_WAIT_MAX_SECS`); a larger one is
    /// cut to it. Default [`DEFAULT_WAIT_MAX`].
    pub fn with_wait_max(mut self, max: Duration) -> Result<Self, ConfigError> {
        if max.is_zero() {
            return Err(ConfigError::Zero("the wait bound"));
        }
        self.wait_max = max;
        Ok(self)
    }

    /// How often `wait_for_job` sends a heartbeat notification, so that a client's idle window
    /// keeps being reset. Default [`DEFAULT_HEARTBEAT`]; tests make it short.
    pub fn with_heartbeat(mut self, interval: Duration) -> Result<Self, ConfigError> {
        if interval.is_zero() {
            return Err(ConfigError::Zero("the heartbeat interval"));
        }
        self.heartbeat = interval;
        Ok(self)
    }

    /// The public origin of the chat (`https://chat.example.com`), so that `start_job` can give
    /// the address of the job in the web UI. Without it `start_job` has no `web_url`.
    pub fn with_public_url(mut self, url: &str) -> Result<Self, ConfigError> {
        let url = url.trim().trim_end_matches('/');
        let has_host = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'));
        if !has_host {
            return Err(ConfigError::BadPublicUrl(url.to_owned()));
        }
        self.public_url = Some(url.to_owned());
        Ok(self)
    }
}

/// What the servers built for each request share.
#[derive(Debug)]
pub(crate) struct Settings {
    public_url: Option<String>,
    wait_max: Duration,
    heartbeat: Duration,
    tool_timeout: Duration,
    waits: Arc<WaitSlots>,
}

impl Settings {
    /// Where the chat shows the job, when the chat's address is known.
    pub(crate) fn web_url(&self, job: ThreadId) -> Option<String> {
        self.public_url
            .as_ref()
            .map(|origin| format!("{origin}/threads/{job}"))
    }
}

/// The routes of the surface: `/mcp`, a [machine route](SurfaceRoutes::machine) guarded by the
/// bearer tokens of `config`.
pub fn routes<P: Ports>(app: Arc<App<P>>, config: McpConfig) -> SurfaceRoutes {
    let McpConfig {
        tokens,
        allowed_hosts,
        allowed_origins,
        public_url,
        wait_max,
        heartbeat,
        wait_max_concurrent,
        wait_max_per_user,
        tool_timeout,
    } = config;
    let settings = Arc::new(Settings {
        public_url,
        wait_max,
        heartbeat,
        tool_timeout,
        waits: WaitSlots::new(wait_max_concurrent, wait_max_per_user),
    });
    let http = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        // No response head before the tool has answered: see "Response framing" in the crate docs.
        .with_json_response(true)
        .with_allowed_hosts(allowed_hosts)
        // With an empty list this refuses every request that carries an `Origin` (browsers do,
        // the CLI clients do not), and a listed origin is let through.
        .with_allowed_origins(allowed_origins)
        .enforce_origin_validation();
    let service = StreamableHttpService::new(
        move || {
            Ok(server::McpServer::new(
                Arc::clone(&app),
                Arc::clone(&settings),
            ))
        },
        Arc::new(NeverSessionManager::default()),
        http,
    );
    let router = Router::new().nest_service(MCP_PATH, service);
    let guard = from_fn_with_state(Arc::new(tokens), auth::require_bearer);
    SurfaceRoutes::new().machine(router, guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_a_scheme_a_host_and_optionally_a_port() {
        for ok in ["https://chat.example.com", "http://localhost:3000"] {
            assert!(is_origin(ok), "{ok}");
        }
        for bad in [
            "",
            "*",
            "chat.example.com",
            "https://chat.example.com/",
            "https://chat.example.com/path",
            "ftp://chat.example.com",
            "https://user@chat.example.com",
        ] {
            assert!(!is_origin(bad), "{bad:?}");
        }
    }
}
