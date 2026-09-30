//! The MCP server surface (ADR 0019): the tools `list_agents`, `start_job`, `get_job`, `answer`
//! and `cancel_job` over MCP streamable HTTP, at `/mcp`, so that Claude Code, opencode or any MCP
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
//! answer {job_id, text}   -> {job_id, state}
//! cancel_job {job_id}     -> {job_id, state, finished}
//! list_agents {}          -> {agents: [{id, name, description}]}
//! ```

mod auth;
mod id;
mod job;
mod server;
mod tools;

use std::sync::Arc;

use axum::Router;
use axum::middleware::from_fn_with_state;
use orch_api::SurfaceRoutes;
use orch_app::App;
use orch_core::ThreadId;
use orch_ports::Ports;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

pub use auth::{McpUser, TokenError, TokenTable};
pub use id::{MAX_CLIENT_REQUEST_ID_BYTES, job_id_for};
pub use job::{Branch, Findings, JobSummary, LastCheck, PullRequest};
pub use tools::{AnswerArgs, CancelJobArgs, GetJobArgs, NoArgs, StartJobArgs, ToolName};

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
    /// The public URL is not an `http` or `https` URL.
    #[error("the public URL {0:?} is not an http(s) URL")]
    BadPublicUrl(String),
}

/// What the surface needs to run.
#[derive(Debug)]
pub struct McpConfig {
    tokens: TokenTable,
    allowed_hosts: Vec<String>,
    public_url: Option<String>,
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
        Ok(McpConfig {
            tokens,
            allowed_hosts,
            public_url: None,
        })
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
        public_url,
    } = config;
    let settings = Arc::new(Settings { public_url });
    let http = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_allowed_hosts(allowed_hosts);
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
