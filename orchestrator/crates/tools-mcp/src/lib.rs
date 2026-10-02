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
//! # Errors
//!
//! | The server | [`ToolServerError`] |
//! |---|---|
//! | cannot be connected to, resets, answers 5xx, 404 or 429, or does not finish the handshake | `Unreachable` |
//! | answers 401 or 403 | `Unauthenticated` |
//! | has not answered the call in time | `TimedOut` |
//! | answers a JSON-RPC error (an unknown tool, bad arguments) | `Remote { code, message }`, the message cut at 1 KiB |
//! | answers something that is not MCP | `Protocol` |
//! | has an endpoint the client cannot use (a URL that is not `http` or `https`, a header a request cannot carry) | `Misconfigured` |
//!
//! A tool's own `isError` is a result, not an error, and is passed through.
//!
//! # Bounds
//!
//! The content of a result is cut at [`MAX_RESULT_BYTES`] before it is returned. What the server
//! sends is read first: an event of a streamed answer over 4 MiB is refused (`Protocol`); a plain
//! JSON answer is read whole (the endpoint's timeout is its bound). What a server says (its tools'
//! descriptions, a result, an error message) is untrusted text.

use std::collections::HashMap;
use std::time::Duration;

use orch_ports::{
    MAX_RESULT_BYTES, ToolCall, ToolCallOutput, ToolDef, ToolServerClient, ToolServerEndpoint,
    ToolServerError,
};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, CallToolResult, MetaObject, RequestMetaObject, Tool};
use rmcp::service::{ClientInitializeError, RunningService, ServiceError};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig, StreamableHttpError,
};
use rmcp::{RoleClient, ServiceExt};
use tokio::time::Instant;

/// The longest the connection of a request may take to be made, whatever the endpoint's timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest event of a streamed answer that is read: 4 MiB, sixteen times the bound on a result,
/// which leaves room for a result a little over the bound to be cut, and not for one of any size.
const MAX_EVENT_BYTES: usize = 16 * MAX_RESULT_BYTES;

/// Header names the transport sets itself. The configuration refuses them too, and so does this
/// crate, so that a caller that built its endpoint in code gets an error and not a request that
/// quietly differs from what it said.
const RESERVED_HEADERS: [&str; 7] = [
    "authorization",
    "accept",
    "content-type",
    "host",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];

/// Why a [`McpToolClient`] cannot be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// The HTTP client could not start (the TLS backend).
    #[error("the HTTP client of the tool servers could not start")]
    Http(#[source] reqwest::Error),
}

/// The [`ToolServerClient`] over MCP. Cheap to clone: clones share one HTTP client.
#[derive(Debug, Clone)]
pub struct McpToolClient {
    http: reqwest::Client,
}

impl McpToolClient {
    /// A client of MCP servers. Installs the `rustls` crypto provider if none is installed yet.
    ///
    /// # Errors
    /// [`BuildError::Http`] when the TLS backend does not start.
    pub fn new() -> Result<Self, BuildError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let http = reqwest::Client::builder()
            // A connection is not kept for the next request: every request is a session of its
            // own, and an idle connection to a server that closed it is a failed first attempt.
            .pool_max_idle_per_host(0)
            .connect_timeout(CONNECT_TIMEOUT)
            // The headers carry the server's credentials: never replayed to a redirect's target.
            .redirect(reqwest::redirect::Policy::none())
            // A server of the deployment's list is reached as configured, not through a proxy of
            // the environment.
            .no_proxy()
            .build()
            .map_err(BuildError::Http)?;
        Ok(McpToolClient { http })
    }

    /// The session of one request: connected and initialized, or why not.
    async fn open(
        &self,
        server: &ToolServerEndpoint,
        deadline: Instant,
    ) -> Result<RunningService<RoleClient, ()>, ToolServerError> {
        let transport = StreamableHttpClientTransport::with_client(
            self.http.clone(),
            transport_config(server)?,
        );
        match tokio::time::timeout_at(deadline, ().serve(transport)).await {
            Ok(Ok(client)) => Ok(client),
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
        let client = self.open(server, deadline).await?;
        let listed = within(server, deadline, async {
            client.list_all_tools().await.map_err(from_service)
        })
        .await;
        drop(client);
        Ok(listed?.into_iter().map(tool_def).collect())
    }

    async fn call_tool(
        &self,
        server: &ToolServerEndpoint,
        call: &ToolCall,
    ) -> Result<ToolCallOutput, ToolServerError> {
        let deadline = Instant::now() + server.timeout;
        let client = self.open(server, deadline).await?;
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
        Ok(output(called?))
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

/// The transport's settings for `server`: its URL, its bearer and its headers.
fn transport_config(
    server: &ToolServerEndpoint,
) -> Result<StreamableHttpClientTransportConfig, ToolServerError> {
    let scheme = server
        .url
        .get(..8)
        .unwrap_or(&server.url)
        .to_ascii_lowercase();
    if !(scheme.starts_with("http://") || scheme.starts_with("https://")) {
        return Err(ToolServerError::Misconfigured(
            "the URL is not an http or https one".to_owned(),
        ));
    }
    let mut headers = HashMap::new();
    for (name, value) in &server.headers {
        if RESERVED_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
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
    let mut config = StreamableHttpClientTransportConfig::with_uri(server.url.clone())
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
        401 | 403 => ToolServerError::Unauthenticated,
        404 | 408 | 429 | 500..=599 => {
            ToolServerError::unreachable(format!("the server answered HTTP {status}"))
        }
        _ => ToolServerError::protocol(format!("the server answered HTTP {status}")),
    }
}

fn from_reqwest(error: &reqwest::Error) -> ToolServerError {
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
