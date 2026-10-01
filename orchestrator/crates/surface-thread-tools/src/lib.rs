//! The thread-tools surface (`thread-tools/v1`): one MCP endpoint per thread, at
//! `/thread-tools/{threadId}/mcp`, for the agent that is working on that thread.
//!
//! An agent whose card lists the extension receives `{url, token, expiresAt}` in the metadata of
//! the A2A message (the A2A adapter mints the token when it sends, see `orch-thread-token`), and
//! calls this endpoint with it as a bearer token. The contract is
//! [`docs/api/thread-tools-v1.md`](../../../../docs/api/thread-tools-v1.md).
//!
//! # Where it sits
//!
//! An adapter over [`orch_app::App`], like every inbound surface (ADR 0009), mounted by the
//! binary with `ORCH_SURFACES=thread-tools` (Cargo feature `surface-thread-tools`). It is a
//! [machine route](orch_api::SurfaceRoutes::machine): outside the identity layer (no cookie,
//! `X-Auth-Request-Email` is never read), guarded by the token, and **not under `/mcp`**, which
//! belongs to the MCP server of ADR 0019 and would swallow it.
//!
//! # The guard
//!
//! Every request is checked before any tool runs, in this order: a bearer token is present; it is
//! a valid token of this orchestrator ([`orch_thread_token::verify`]: size, header, key,
//! signature, claims, issuer, audience, lifetime); its thread is the path's; the thread exists
//! and the caller is its agent. Every failure of these is the same `401` with
//! `WWW-Authenticate: Bearer error="invalid_token"` and no detail, with nothing written and
//! nothing called. The server then validates the `Host` header ([`ThreadToolsConfig`] requires
//! the list of hosts it accepts) and answers `403` for any other.
//!
//! # Stateless
//!
//! `rmcp`'s `StreamableHttpService` runs with `legacy_session_mode = false` and a session
//! manager that keeps nothing: a fresh server is built for every request, so any replica serves
//! any request, and a call is answered with one `application/json` response (no event stream
//! unless a tool reports progress).
//!
//! # The tools
//!
//! `tools/list` is the built-in tools ([`ThreadTool`], a closed enum: today `get_ui_catalog`)
//! and then each provider's ([`ThreadToolProvider`], added with
//! [`ThreadToolsConfig::with_provider`]), computed per request. `tools/call` offers the name to
//! the built-ins, then to the providers in order; a name nobody owns is the JSON-RPC error
//! `-32602`. Later slices add providers (the tools of attached MCP servers, `ask_agent`); they do
//! not change the token or the route.

mod guard;
mod provider;
mod server;
mod tools;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::middleware::from_fn_with_state;
use orch_api::{SurfaceRoutes, is_host_authority};
use orch_app::App;
use orch_ports::Ports;
use orch_thread_token::ThreadToolsKeys;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::guard::GuardState;
use crate::provider::DynProvider;

pub use guard::ROUTE;
pub use provider::{ProgressSink, ThreadToolProvider, ToolCtx};
pub use tools::ThreadTool;

/// How long a built-in tool may take before it is cut off. A machine route has no request
/// timeout of its own.
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a [`ThreadToolsConfig`] cannot be built.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    /// No host is allowed: every request would be refused. `rmcp` would fall back to loopback
    /// only; the surface asks for the list instead of choosing one.
    #[error("no allowed host is configured")]
    NoAllowedHosts,
    /// A host entry that is not a host name, an address, or either with a port: `https://…`,
    /// `*`, a path or credentials never match the `Host` header, so they would refuse everyone
    /// (or, worse, look like a rule that is not one).
    #[error("the allowed host {0:?} is not a host name or address, with or without a port")]
    BadHost(String),
    /// A tool timeout of zero: every built-in tool would be cut off at once.
    #[error("the tool timeout must not be zero")]
    ZeroTimeout,
}

/// What the surface needs to run.
pub struct ThreadToolsConfig {
    keys: ThreadToolsKeys,
    allowed_hosts: Vec<String>,
    tool_timeout: Duration,
    providers: Vec<Arc<dyn DynProvider>>,
}

impl std::fmt::Debug for ThreadToolsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadToolsConfig")
            .field("keys", &self.keys)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("tool_timeout", &self.tool_timeout)
            .field("providers", &self.providers.len())
            .finish()
    }
}

impl ThreadToolsConfig {
    /// A configuration with the keys the tokens are verified with and the `Host` values the
    /// server accepts (`orchestrator:8080`, `orch.example.com`; a name without a port matches any
    /// port).
    ///
    /// # Errors
    ///
    /// [`ConfigError`] for an empty list, or an entry that is not a host.
    pub fn new(
        keys: ThreadToolsKeys,
        allowed_hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ConfigError> {
        let allowed_hosts: Vec<String> = allowed_hosts
            .into_iter()
            .map(|h| h.into().trim().to_owned())
            .collect();
        if allowed_hosts.is_empty() {
            return Err(ConfigError::NoAllowedHosts);
        }
        if let Some(bad) = allowed_hosts.iter().find(|h| !is_host_authority(h)) {
            return Err(ConfigError::BadHost(bad.clone()));
        }
        Ok(ThreadToolsConfig {
            keys,
            allowed_hosts,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            providers: Vec::new(),
        })
    }

    /// How long a built-in tool may take (default [`DEFAULT_TOOL_TIMEOUT`]). A provider's tools
    /// bound themselves.
    ///
    /// # Errors
    ///
    /// [`ConfigError::ZeroTimeout`] for a zero duration.
    pub fn with_tool_timeout(mut self, timeout: Duration) -> Result<Self, ConfigError> {
        if timeout.is_zero() {
            return Err(ConfigError::ZeroTimeout);
        }
        self.tool_timeout = timeout;
        Ok(self)
    }

    /// Adds a provider of tools. Its tools come after the built-in ones and after those of the
    /// providers added before it.
    #[must_use]
    pub fn with_provider(mut self, provider: impl ThreadToolProvider) -> Self {
        self.providers.push(Arc::new(provider));
        self
    }
}

/// What the servers built for each request share.
pub(crate) struct Settings {
    pub(crate) providers: Vec<Arc<dyn DynProvider>>,
    pub(crate) tool_timeout: Duration,
}

/// The routes of the surface: [`ROUTE`], a [machine route](SurfaceRoutes::machine) guarded by the
/// token.
pub fn routes<P: Ports>(app: Arc<App<P>>, config: ThreadToolsConfig) -> SurfaceRoutes {
    let ThreadToolsConfig {
        keys,
        allowed_hosts,
        tool_timeout,
        providers,
    } = config;
    let settings = Arc::new(Settings {
        providers,
        tool_timeout,
    });
    let http = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        // One JSON response when the tool has answered: see the crate docs.
        .with_json_response(true)
        .with_allowed_hosts(allowed_hosts)
        // An agent is not a browser: a request that carries an `Origin` is refused.
        .with_allowed_origins(Vec::<String>::new())
        .enforce_origin_validation();
    let service = StreamableHttpService::new(
        {
            let app = Arc::clone(&app);
            move || {
                Ok(server::ThreadToolsServer::new(
                    Arc::clone(&app),
                    Arc::clone(&settings),
                ))
            }
        },
        Arc::new(NeverSessionManager::default()),
        http,
    );
    // The service ignores the path it is called at; the guard reads the thread from it.
    let router = Router::new().route_service(ROUTE, service);
    let guard = from_fn_with_state(GuardState { app, keys }, guard::require_grant::<P>);
    SurfaceRoutes::new().machine(router, guard)
}
