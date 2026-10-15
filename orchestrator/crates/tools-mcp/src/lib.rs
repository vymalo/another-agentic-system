//! `ToolServerClient` over MCP: the orchestrator as a client of an MCP server (ADR 0024).
//!
//! One implementation of the port in [`orch_ports`] on `rmcp`'s client over **streamable HTTP**.
//! The relay of the thread-tools endpoint lists the tools of a server the deployment configured and
//! calls one for an agent; this crate speaks to the server, and the port's types are all the relay
//! sees.
//!
//! # What one request is
//!
//! Every request is its own MCP session: connect, `initialize`, the one request (`tools/list`, all
//! its pages, or `tools/call`), and the session is let go. Nothing is held between requests, so any
//! replica serves any request (ADR 0001) and a server that restarted costs nothing. The transport
//! accepts a server that keeps no session (it answers without an `Mcp-Session-Id`) as well as one
//! that does.
//!
//! The endpoint's `timeout` bounds the **whole** request. A handshake that has not finished by then
//! is `Unreachable` (nothing answered), a call that has not answered by then is `TimedOut`. A call is
//! cancelled by dropping its future.
//!
//! # Credentials
//!
//! `Authorization: Bearer <bearer>` and the endpoint's headers go on every request to the server
//! and nowhere else: the HTTP client follows **no redirect** (a redirect would replay the headers
//! to another host; the server's final URL is the one to configure), the header values are marked
//! sensitive, and no error holds one.
//!
//! # Servers people name
//!
//! [`McpToolClient::guarded`] is the client for a server a person typed, a stranger's URL
//! (ADR 0046, decision 5). It is the same client with the egress guard in front of every connection:
//!
//! 1. the endpoint's URL goes through [`EgressPolicy::check_url`](orch_ports::EgressPolicy::check_url)
//!    and the client connects to the **normal form** it returns, so what was judged is what is used;
//! 2. an address written in the URL is judged at once
//!    ([`check_resolved`](orch_ports::EgressPolicy::check_resolved)): the connector of the HTTP client
//!    does not ask a resolver about an address;
//! 3. a name is resolved by [`HostLookup`] **once**, through the client's own resolver, and the answer
//!    is judged ([`check_connection`](orch_ports::EgressPolicy::check_connection)); the connection is
//!    made to the addresses that passed and to no other, so there is no second lookup to rebind;
//! 4. no idle connection is kept (every request is judged again), no redirect is followed and no proxy
//!    is used (a proxy connects where it likes);
//! 5. a connector layer (`peer.rs`) judges **every connection the client is asked to make**, on the
//!    destination before it is made and on the peer's address after, so an address written in a URL
//!    is judged even if redirects were ever turned on;
//! 6. the lookup of a name is bounded by [`LOOKUP_TIMEOUT`](orch_ports::LOOKUP_TIMEOUT) inside the
//!    resolver. `getaddrinfo` runs on the blocking pool and **that thread cannot be cancelled**: the
//!    timeout frees the request, not the thread, which the operating system's resolver frees when it
//!    gives up. A deployment bounds how many requests to people's servers are in flight (the relay
//!    does), so the threads such lookups can pin are bounded too.
//!
//! A refusal is [`ToolServerError::EgressRefused`] with the rule, and **nothing was sent**: the
//! handshake, and with it the credential, never leave. The client built by [`McpToolClient::new`] is
//! the deployment's own and judges nothing: its servers are the operator's choice.
//!
//! # Errors
//!
//! | The server | [`ToolServerError`] |
//! |---|---|
//! | cannot be connected to, resets, answers 5xx, 404 or 429, or does not finish the handshake | `Unreachable` |
//! | answers 401, or 403 with a `WWW-Authenticate` challenge | `Unauthenticated` |
//! | answers 403 with no challenge | `Forbidden` |
//! | has not answered the call in time | `TimedOut` |
//! | answers a JSON-RPC error (an unknown tool, bad arguments) | `Remote { code, message }`, the message cut at 1 KiB |
//! | answers something that is not MCP | `Protocol` |
//! | has an endpoint the client cannot use (a URL that is not `http` or `https`, a header a request cannot carry) | `Misconfigured` |
//! | is refused by the egress guard (a guarded client only): its URL, or an address its name resolved to | `EgressRefused`, nothing sent |
//!
//! A tool's own `isError` is a result, not an error, and is passed through.
//!
//! # Bounds
//!
//! The content of a result is cut at [`MAX_RESULT_BYTES`] before it is returned. What the server
//! sends is read first, and the client of the servers people name reads **at most
//! [`MAX_RESPONSE_BYTES`] of any one response** (4 MiB: the body of a JSON answer, of an error, the
//! whole of an event stream), on the wire: `wire.rs` explains why that needs a backend of its own
//! under rmcp. A longer answer fails the request (`Protocol`) and the connection is closed, so a
//! server cannot make the orchestrator buffer what it likes. The listing of such a server stops at
//! [`MAX_LIST_PAGES`] pages and
//! [`MAX_USER_SERVER_TOOLS`](orch_ports::MAX_USER_SERVER_TOOLS) tools, and a tool whose schema is
//! over [`MAX_USER_TOOL_SCHEMA_BYTES`](orch_ports::MAX_USER_TOOL_SCHEMA_BYTES) is left out; the log
//! says that it happened with counts, never with a word of the server's. The deployment's own client
//! reads as rmcp does: an event of a streamed answer over 4 MiB is refused, a plain JSON answer is
//! read whole (the endpoint's timeout is its bound).
//!
//! What a server says (its tools' descriptions, a result, an error message) is untrusted text. No
//! error this crate makes holds a URL: every `reqwest::Error` has its URL taken out
//! (`without_url`) before it is kept, so a secret in a person's URL path is in no log line, for the
//! guarded client and for the deployment's.

mod peer;
mod wire;

use std::borrow::Cow;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use orch_ports::{
    EgressPolicy, EgressRefusal, LOOKUP_TIMEOUT, LookupFailure, MAX_RESULT_BYTES,
    MAX_USER_SERVER_TOOLS, MAX_USER_TOOL_SCHEMA_BYTES, ToolCall, ToolCallOutput, ToolDef,
    ToolServerClient, ToolServerEndpoint, ToolServerError, is_reserved_header, judge_lookup,
};
pub use orch_ports::{HostLookup, LookupFuture};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, MetaObject, PaginatedRequestParams, RequestMetaObject,
    Tool,
};
use rmcp::service::{ClientInitializeError, RunningService, ServiceError};
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    StreamableHttpError,
};
use rmcp::{RoleClient, ServiceExt};
use tokio::time::Instant;

use crate::peer::PeerGuard;
use crate::wire::{ANSWER_TOO_LARGE, BodyError, Cut, WireClient};

/// The longest the connection of a request may take to be made, whatever the endpoint's timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The most bytes of any one response the client of the servers people name reads: 4 MiB (sixteen
/// times the bound on a result). More fails the request and closes the connection.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// The most pages of a listing the client of the servers people name follows.
pub const MAX_LIST_PAGES: usize = 10;

/// The largest event of a streamed answer that is read: 4 MiB, sixteen times the bound on a result,
/// which leaves room for a result a little over the bound to be cut, and not for one of any size.
const MAX_EVENT_BYTES: usize = 16 * MAX_RESULT_BYTES;

/// Why a [`McpToolClient`] cannot be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// The HTTP client could not start (the TLS backend).
    #[error("the HTTP client of the tool servers could not start")]
    Http(#[source] reqwest::Error),
}

/// [`HostLookup`] over the operating system's resolver (`getaddrinfo`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLookup;

impl HostLookup for SystemLookup {
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a> {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host, 0)).await?;
            Ok(addrs.map(|addr| addr.ip()).collect())
        })
    }
}

/// The bounds of a guarded client. The defaults are the module's constants; a test names smaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct GuardedLimits {
    /// The most bytes of one response ([`MAX_RESPONSE_BYTES`]).
    pub max_response_bytes: usize,
    /// How long a lookup may take ([`LOOKUP_TIMEOUT`]).
    pub lookup_timeout: Duration,
    /// The most pages of a listing ([`MAX_LIST_PAGES`]).
    pub max_pages: usize,
    /// The most tools of a listing ([`MAX_USER_SERVER_TOOLS`]).
    pub max_tools: usize,
}

impl Default for GuardedLimits {
    fn default() -> Self {
        GuardedLimits {
            max_response_bytes: MAX_RESPONSE_BYTES,
            lookup_timeout: LOOKUP_TIMEOUT,
            max_pages: MAX_LIST_PAGES,
            max_tools: MAX_USER_SERVER_TOOLS,
        }
    }
}

impl GuardedLimits {
    /// The most bytes of one response.
    #[must_use]
    pub const fn with_max_response_bytes(mut self, bytes: usize) -> Self {
        self.max_response_bytes = bytes;
        self
    }

    /// How long a lookup may take.
    #[must_use]
    pub const fn with_lookup_timeout(mut self, timeout: Duration) -> Self {
        self.lookup_timeout = timeout;
        self
    }

    /// The most pages of a listing.
    #[must_use]
    pub const fn with_max_pages(mut self, pages: usize) -> Self {
        self.max_pages = pages;
        self
    }

    /// The most tools of a listing.
    #[must_use]
    pub const fn with_max_tools(mut self, tools: usize) -> Self {
        self.max_tools = tools;
        self
    }
}

/// The resolver of a guarded client: resolves once, judges the answer, hands the client only the
/// addresses that passed.
struct GuardedResolver {
    policy: EgressPolicy,
    lookup: Arc<dyn HostLookup>,
    timeout: Duration,
}

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy.clone();
        let lookup = Arc::clone(&self.lookup);
        let timeout = self.timeout;
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let bare = host.trim_start_matches('[').trim_end_matches(']');
            let answer = match bare.parse::<IpAddr>() {
                Ok(ip) => Ok(vec![ip]),
                // A lookup that never answers is cut here; the thread `getaddrinfo` runs on is not
                // (module docs).
                Err(_) => match tokio::time::timeout(timeout, lookup.lookup(bare)).await {
                    Ok(answer) => answer,
                    Err(_) => return Err(LookupFailure::TimedOut.into()),
                },
            };
            let checked = judge_lookup(&policy, bare, answer)?;
            let addrs: Addrs = Box::new(checked.into_vec().into_iter());
            Ok(addrs)
        })
    }
}

/// The [`ToolServerClient`] over MCP. Cheap to clone: clones share one HTTP client.
#[derive(Debug, Clone)]
pub struct McpToolClient {
    http: reqwest::Client,
    /// The policy and the bounds of a guarded client; `None` for the deployment's own.
    guard: Option<Guard>,
}

/// What makes a client guarded.
#[derive(Debug, Clone)]
struct Guard {
    policy: EgressPolicy,
    limits: GuardedLimits,
}

impl McpToolClient {
    /// A client of the MCP servers the deployment lists. Installs the `rustls` crypto provider if
    /// none is installed yet. It judges no address: use [`guarded`](Self::guarded) for a server a
    /// person named.
    ///
    /// # Errors
    /// [`BuildError::Http`] when the TLS backend does not start.
    pub fn new() -> Result<Self, BuildError> {
        Ok(McpToolClient {
            http: Self::builder().build().map_err(BuildError::Http)?,
            guard: None,
        })
    }

    /// A client of the MCP servers people name: [`new`](Self::new) with the egress guard in front
    /// of every connection (see the module's docs), names resolved by the operating system.
    ///
    /// # Errors
    /// [`BuildError::Http`] when the TLS backend does not start.
    pub fn guarded(policy: EgressPolicy) -> Result<Self, BuildError> {
        Self::guarded_with_lookup(policy, SystemLookup)
    }

    /// [`guarded`](Self::guarded), with `lookup` in place of the operating system's resolver: a
    /// test that maps a name to a loopback address (DNS rebinding), or a deployment with a resolver
    /// of its own. The guard judges what `lookup` answers.
    ///
    /// # Errors
    /// [`BuildError::Http`] when the TLS backend does not start.
    pub fn guarded_with_lookup(
        policy: EgressPolicy,
        lookup: impl HostLookup,
    ) -> Result<Self, BuildError> {
        Self::guarded_with_limits(policy, lookup, GuardedLimits::default())
    }

    /// [`guarded_with_lookup`](Self::guarded_with_lookup) with bounds other than the defaults
    /// ([`GuardedLimits`]): a test that wants a response cut at a few kilobytes.
    ///
    /// # Errors
    /// [`BuildError::Http`] when the TLS backend does not start.
    pub fn guarded_with_limits(
        policy: EgressPolicy,
        lookup: impl HostLookup,
        limits: GuardedLimits,
    ) -> Result<Self, BuildError> {
        let resolver = GuardedResolver {
            policy: policy.clone(),
            lookup: Arc::new(lookup),
            timeout: limits.lookup_timeout,
        };
        let http = Self::builder()
            .dns_resolver(resolver)
            .connector_layer(PeerGuard::new(policy.clone()))
            .build()
            .map_err(|e| BuildError::Http(e.without_url()))?;
        Ok(McpToolClient {
            http,
            guard: Some(Guard { policy, limits }),
        })
    }

    fn builder() -> reqwest::ClientBuilder {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        reqwest::Client::builder()
            // A connection is not kept for the next request: every request is a session of its
            // own, and an idle connection to a server that closed it is a failed first attempt (and,
            // for a guarded client, a connection to an address that was judged once).
            .pool_max_idle_per_host(0)
            .connect_timeout(CONNECT_TIMEOUT)
            // The headers carry the server's credentials: never replayed to a redirect's target.
            .redirect(reqwest::redirect::Policy::none())
            // A server is reached as configured, not through a proxy of the environment (which
            // would connect where it likes).
            .no_proxy()
    }

    /// The URL to connect to: as given for the deployment's client, the guard's normal form of it
    /// for a guarded one, whose address, when the URL writes one, is judged now (the connector asks
    /// no resolver about an address).
    fn admitted<'a>(
        &self,
        server: &'a ToolServerEndpoint,
    ) -> Result<Cow<'a, str>, ToolServerError> {
        let Some(Guard { policy, .. }) = &self.guard else {
            return Ok(Cow::Borrowed(&server.url));
        };
        let target = policy
            .check_url(&server.url)
            .map_err(ToolServerError::EgressRefused)?;
        if let Some(addr) = target.literal_addr() {
            policy
                .check_resolved(&target, &[addr])
                .map_err(ToolServerError::EgressRefused)?;
        }
        Ok(Cow::Owned(target.url().to_owned()))
    }

    /// The session of one request: connected and initialized, or why not.
    async fn open(
        &self,
        server: &ToolServerEndpoint,
        deadline: Instant,
    ) -> Result<(RunningService<RoleClient, ()>, Cut), ToolServerError> {
        let url = self.admitted(server)?;
        let max_body = self.guard.as_ref().map(|g| g.limits.max_response_bytes);
        let wire = WireClient::new(self.http.clone(), max_body);
        let cut = wire.cut();
        let transport =
            StreamableHttpClientTransport::with_client(wire, transport_config(server, &url)?);
        match tokio::time::timeout_at(deadline, ().serve(transport)).await {
            Ok(Ok(client)) => Ok((client, cut)),
            Ok(Err(_)) if cut.tripped() => Err(too_large_answer()),
            Ok(Err(error)) => Err(from_handshake(&error).with_source(error)),
            Err(_) => Err(ToolServerError::unreachable(format!(
                "no answer to the handshake within {} s",
                server.timeout.as_secs().max(1)
            ))),
        }
    }
}

impl ToolServerClient for McpToolClient {
    async fn list_tools(
        &self,
        server: &ToolServerEndpoint,
    ) -> Result<Vec<ToolDef>, ToolServerError> {
        let deadline = Instant::now() + server.timeout;
        let (client, cut) = self.open(server, deadline).await?;
        let listed = within(server, deadline, async {
            match &self.guard {
                // a stranger's listing: a few pages and a few tools, whatever it says
                Some(Guard { limits, .. }) => list_bounded(&client, &server.id, limits).await,
                None => client.list_all_tools().await.map_err(from_service),
            }
        })
        .await;
        drop(client);
        Ok(answer(listed, &cut)?.into_iter().map(tool_def).collect())
    }

    async fn call_tool(
        &self,
        server: &ToolServerEndpoint,
        call: &ToolCall,
    ) -> Result<ToolCallOutput, ToolServerError> {
        let deadline = Instant::now() + server.timeout;
        let (client, cut) = self.open(server, deadline).await?;
        let mut params =
            CallToolRequestParams::new(call.name.clone()).with_arguments(call.arguments.clone());
        params.meta = call
            .meta
            .clone()
            .map(|meta| RequestMetaObject(MetaObject(meta)));
        let called = within(server, deadline, async {
            client.call_tool(params).await.map_err(from_service)
        })
        .await;
        drop(client);
        Ok(output(answer(called, &cut)?))
    }
}

/// Runs `request` until `deadline`: a request that is not answered by then is `TimedOut`.
async fn within<T>(
    server: &ToolServerEndpoint,
    deadline: Instant,
    request: impl Future<Output = Result<T, ToolServerError>>,
) -> Result<T, ToolServerError> {
    match tokio::time::timeout_at(deadline, request).await {
        Ok(answer) => answer,
        Err(_) => Err(ToolServerError::TimedOut {
            after: server.timeout,
        }),
    }
}

/// `result`, unless a response of the session was over the bound: whatever rmcp made of a body
/// that was cut in the middle of a stream, the answer is that it was larger than allowed.
fn answer<T>(result: Result<T, ToolServerError>, cut: &Cut) -> Result<T, ToolServerError> {
    match result {
        Err(_) if cut.tripped() => Err(too_large_answer()),
        other => other,
    }
}

fn too_large_answer() -> ToolServerError {
    ToolServerError::protocol(ANSWER_TOO_LARGE)
}

/// The tools of a stranger's server: at most `limits.max_pages` pages and `limits.max_tools`
/// tools, and none whose schema is over the bound. Past the first two the listing is cut where it
/// is and the log says so; the log holds counts, never a word the server chose.
async fn list_bounded(
    client: &RunningService<RoleClient, ()>,
    server: &str,
    limits: &GuardedLimits,
) -> Result<Vec<Tool>, ToolServerError> {
    let mut tools: Vec<Tool> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0_usize;
    let mut too_big = 0_usize;
    loop {
        let page = client
            .list_tools(Some(
                PaginatedRequestParams::default().with_cursor(cursor.take()),
            ))
            .await
            .map_err(from_service)?;
        pages += 1;
        let mut more_than_taken = false;
        for tool in page.tools {
            if tools.len() >= limits.max_tools {
                more_than_taken = true;
                break;
            }
            if schema_too_big(&tool) {
                too_big += 1;
            } else {
                tools.push(tool);
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() && !more_than_taken {
            break;
        }
        if more_than_taken || pages >= limits.max_pages || tools.len() >= limits.max_tools {
            tracing::warn!(
                %server,
                pages,
                tools = tools.len(),
                "the listing of a server of a person's was cut: it has more pages or tools than are taken"
            );
            break;
        }
    }
    if too_big > 0 {
        tracing::warn!(
            %server,
            tools = too_big,
            "tools of a server of a person's whose schema is over the limit were left out"
        );
    }
    Ok(tools)
}

/// Whether the JSON of a tool's input or output schema is over
/// [`MAX_USER_TOOL_SCHEMA_BYTES`].
fn schema_too_big(tool: &Tool) -> bool {
    let size = |schema: &serde_json::Map<String, serde_json::Value>| {
        serde_json::to_vec(schema).map_or(usize::MAX, |bytes| bytes.len())
    };
    size(&tool.input_schema) > MAX_USER_TOOL_SCHEMA_BYTES
        || tool
            .output_schema
            .as_ref()
            .is_some_and(|schema| size(schema) > MAX_USER_TOOL_SCHEMA_BYTES)
}

/// The transport's settings for `server`, to connect to `url` (its own, or the guard's normal form
/// of it): the URL, the bearer and the headers.
fn transport_config(
    server: &ToolServerEndpoint,
    url: &str,
) -> Result<StreamableHttpClientTransportConfig, ToolServerError> {
    let scheme = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    if !(scheme.starts_with("http://") || scheme.starts_with("https://")) {
        return Err(ToolServerError::Misconfigured(
            "the URL is not an http or https one".to_owned(),
        ));
    }
    let mut headers = HashMap::new();
    for (name, value) in &server.headers {
        // the names the transport sets itself: refused here too (the configuration refuses them), so
        // that a caller that built its endpoint in code gets an error and not a request that
        // quietly differs from what it said
        if is_reserved_header(name) {
            return Err(ToolServerError::Misconfigured(format!(
                "the header {name} is one the transport sets itself"
            )));
        }
        let (Ok(header), Ok(mut value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value.expose()),
        ) else {
            return Err(ToolServerError::Misconfigured(format!(
                "the header {name} is not one a request can carry"
            )));
        };
        value.set_sensitive(true);
        headers.insert(header, value);
    }
    let mut config = StreamableHttpClientTransportConfig::with_uri(url.to_owned())
        .custom_headers(headers)
        .max_sse_event_size(MAX_EVENT_BYTES);
    // A server that keeps no session answers `initialize` without an `Mcp-Session-Id`: accepted
    // (the thread-tools endpoint of another orchestrator is one).
    config.allow_stateless = true;
    if let Some(bearer) = &server.bearer {
        config = config.auth_header(bearer.expose());
    }
    Ok(config)
}

/// A tool the server offers, in the port's terms. The icons it lists are dropped (open question 38).
fn tool_def(tool: Tool) -> ToolDef {
    ToolDef {
        name: tool.name.into_owned(),
        title: tool.title,
        description: tool.description.map(std::borrow::Cow::into_owned),
        input_schema: (*tool.input_schema).clone(),
        output_schema: tool.output_schema.map(|schema| (*schema).clone()),
        annotations: tool
            .annotations
            .and_then(|annotations| serde_json::to_value(annotations).ok()),
    }
}

/// What the server answered, in the port's terms, cut at [`MAX_RESULT_BYTES`].
fn output(result: CallToolResult) -> ToolCallOutput {
    ToolCallOutput {
        content: result
            .content
            .iter()
            .filter_map(|block| serde_json::to_value(block).ok())
            .collect(),
        structured: result.structured_content,
        is_error: result.is_error.unwrap_or(false),
        truncated: false,
    }
    .bounded(MAX_RESULT_BYTES)
}

/// A failure of a request that was made, in the port's terms.
fn from_service(error: ServiceError) -> ToolServerError {
    let mapped = match &error {
        ServiceError::McpError(e) => return ToolServerError::remote(e.code.0, &e.message),
        ServiceError::Timeout { timeout } => {
            return ToolServerError::TimedOut { after: *timeout };
        }
        ServiceError::TransportSend(transport) => from_error(transport)
            .unwrap_or_else(|| ToolServerError::unreachable("the request could not be sent")),
        ServiceError::TransportClosed => ToolServerError::unreachable("the connection closed"),
        ServiceError::Cancelled { .. } => ToolServerError::unreachable("the request was cancelled"),
        _ => ToolServerError::protocol("the server answered something that is not a result"),
    };
    mapped.with_source(error)
}

/// A failure of the handshake, in the port's terms.
fn from_handshake(error: &ClientInitializeError) -> ToolServerError {
    match error {
        ClientInitializeError::TransportError { error, .. } => from_error(error)
            .unwrap_or_else(|| ToolServerError::unreachable("the handshake could not be sent")),
        ClientInitializeError::ConnectionClosed(_) => {
            ToolServerError::unreachable("the server closed the connection during the handshake")
        }
        ClientInitializeError::JsonRpcError(e) => ToolServerError::remote(e.code.0, &e.message),
        _ => ToolServerError::protocol("the MCP handshake failed"),
    }
}

/// What the transport error under `error` says about the server, when it says anything: found by
/// walking the chain of sources for the HTTP client's own errors.
fn from_error(error: &(dyn std::error::Error + 'static)) -> Option<ToolServerError> {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(http) = e.downcast_ref::<StreamableHttpError<reqwest::Error>>() {
            return Some(from_http(http));
        }
        if let Some(http) = e.downcast_ref::<reqwest::Error>() {
            return Some(from_reqwest(http));
        }
        current = e.source();
    }
    None
}

fn from_http(error: &StreamableHttpError<reqwest::Error>) -> ToolServerError {
    match error {
        StreamableHttpError::AuthRequired(_) | StreamableHttpError::InsufficientScope(_) => {
            ToolServerError::Unauthenticated
        }
        StreamableHttpError::Client(e) => from_reqwest(e),
        StreamableHttpError::Sse(SseError::Body(body))
            if matches!(body.downcast_ref::<BodyError>(), Some(BodyError::TooLarge)) =>
        {
            too_large_answer()
        }
        StreamableHttpError::Io(_)
        | StreamableHttpError::UnexpectedEndOfStream
        | StreamableHttpError::TransportChannelClosed
        | StreamableHttpError::SessionExpired
        | StreamableHttpError::SessionRecoveryTimeout
        | StreamableHttpError::ControlRequestTimeout => {
            ToolServerError::unreachable("the connection to the server broke")
        }
        // A status that is not a 401 with a challenge reaches us as text, `HTTP <status>: <body>`
        // (rmcp 3.5): the status is read from it, the body (untrusted) is not repeated anywhere.
        StreamableHttpError::UnexpectedServerResponse(message) if message == ANSWER_TOO_LARGE => {
            too_large_answer()
        }
        StreamableHttpError::UnexpectedServerResponse(message) => match status_in(message) {
            Some(status) => from_status(status),
            None => ToolServerError::protocol("the server did not answer as MCP"),
        },
        StreamableHttpError::ReservedHeaderConflict(name) => ToolServerError::Misconfigured(
            format!("the header {name} is one the transport sets itself"),
        ),
        _ => ToolServerError::protocol("the server did not answer as MCP"),
    }
}

/// The status of rmcp's `HTTP <status>: <body>` message.
fn status_in(message: &str) -> Option<u16> {
    message
        .strip_prefix("HTTP ")?
        .get(..3)?
        .parse()
        .ok()
        .filter(|status| (100..600).contains(status))
}

/// What an HTTP status that is not a success says about the server.
fn from_status(status: u16) -> ToolServerError {
    match status {
        401 => ToolServerError::Unauthenticated,
        // a 403 with a challenge reached us as `InsufficientScope`: this one carries none
        403 => ToolServerError::Forbidden,
        404 | 408 | 429 | 500..=599 => {
            ToolServerError::unreachable(format!("the server answered HTTP {status}"))
        }
        _ => ToolServerError::protocol(format!("the server answered HTTP {status}")),
    }
}

/// The refusal of the egress guard under `error`, when the resolver hook of a guarded client made
/// the connection fail: the hook returns it as the error of the lookup, which the HTTP client keeps
/// as the cause of its connect error.
fn refusal_in(error: &(dyn std::error::Error + 'static)) -> Option<EgressRefusal> {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(refusal) = e.downcast_ref::<EgressRefusal>() {
            return Some(*refusal);
        }
        current = e.source();
    }
    None
}

fn from_reqwest(error: &reqwest::Error) -> ToolServerError {
    match refusal_in(error) {
        // a name that resolves to nothing is a server that cannot be reached, not one that is refused
        Some(EgressRefusal::NoAddress) => {
            return ToolServerError::unreachable("the host did not resolve to an address");
        }
        Some(refusal) => return ToolServerError::EgressRefused(refusal),
        None => {}
    }
    if let Some(status) = error.status() {
        return from_status(status.as_u16());
    }
    if error.is_decode() || (error.is_body() && !error.is_timeout()) {
        return ToolServerError::protocol("the server's answer could not be read");
    }
    let what = if error.is_timeout() {
        "the connection timed out"
    } else if error.is_connect() {
        "the connection was refused or could not be made"
    } else {
        "the request could not be made"
    };
    ToolServerError::unreachable(what)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// The HTTP client inside a guarded client has the connector layer on it: a request sent through
    /// it, **without** the judgement of the URL that `list_tools` makes first, is still refused
    /// before a connection is made to a literal private address. (What a redirect, were it ever
    /// followed, would ask of the connector.)
    #[tokio::test]
    async fn the_http_client_of_a_guarded_client_refuses_a_private_literal_on_its_own() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&connections);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                held.push(socket);
            }
        });
        let client = McpToolClient::guarded(EgressPolicy::new().allowing_http()).unwrap();
        let error = client
            .http
            .post(format!("http://{addr}/mcp"))
            .header("authorization", "Bearer SENTINEL-never-sent")
            // a request that is let through is held by the tripwire: this ends it
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_connect(), "{error:?}");
        assert!(refusal_in(&error).is_some(), "{error:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            connections.load(Ordering::SeqCst),
            0,
            "a connection was made"
        );
        // and the deployment's client, which judges nothing, does connect
        let plain = McpToolClient::new().unwrap();
        let _ = plain
            .http
            .get(format!("http://{addr}/mcp"))
            .timeout(Duration::from_millis(200))
            .send()
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }
}
