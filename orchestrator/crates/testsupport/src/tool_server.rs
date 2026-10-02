//! A tool server for tests: a real MCP server over streamable HTTP, on a port of its own, with the
//! four tools of the tool-server testkit (`orch_ports::testkit::tool_server`), a bearer check and
//! a journal of every request it served. It is what a client of an MCP server is tested against
//! (`orch-tools-mcp`, and the relay of the thread-tools endpoint after it).
//!
//! | Tool | Takes | Answers |
//! |---|---|---|
//! | `echo` | any arguments | its arguments, as one text block (their JSON) and as the structured result |
//! | `fail` | nothing | `isError: true`, "the tool failed on purpose" |
//! | `slow` | nothing | nothing, for five minutes unless the caller goes away |
//! | `big` | `bytes`, an integer | one text block of that many `x` |
//!
//! A request with no `Authorization: Bearer <token>` of the one the server wants, or without a
//! header it was told to require, is answered 401 (with `WWW-Authenticate: Bearer`, as a real
//! server does, unless asked not to) before MCP is spoken.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use orch_ports::testkit::tool_server::{FAIL_TEXT, SeenRequest};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Map, Value, json};
use tokio_util::sync::{CancellationToken, DropGuard};

/// How long the `slow` tool waits for its caller to give up.
const SLOW_FOR: Duration = Duration::from_secs(300);

/// The path the server is mounted at.
const PATH: &str = "/mcp";

struct Shared {
    bearer: Option<String>,
    required_headers: Vec<(String, String)>,
    www_authenticate: bool,
    seen: Mutex<Vec<SeenRequest>>,
}

/// How a [`FakeToolServer`] is set up. Without any of it, it wants no credentials, keeps no session
/// (a fresh server for every request, one JSON answer, like the thread-tools endpoint) and
/// challenges a refused request with `WWW-Authenticate`.
#[derive(Debug, Clone)]
pub struct FakeToolServerOptions {
    /// The bearer token the server wants.
    pub bearer: Option<String>,
    /// Headers the server wants, name and value.
    pub required_headers: Vec<(String, String)>,
    /// Keeps a session per client and answers on an event stream, as a server that holds state
    /// does (`Mcp-Session-Id`), in place of the stateless JSON of the default.
    pub stateful: bool,
    /// Sends `WWW-Authenticate: Bearer` with a 401 (what a real server does; some do not).
    pub www_authenticate: bool,
}

impl Default for FakeToolServerOptions {
    fn default() -> Self {
        FakeToolServerOptions {
            bearer: None,
            required_headers: Vec::new(),
            stateful: false,
            www_authenticate: true,
        }
    }
}

impl FakeToolServerOptions {
    /// Wants `Authorization: Bearer <bearer>`.
    #[must_use]
    pub fn bearer(mut self, bearer: impl Into<String>) -> Self {
        self.bearer = Some(bearer.into());
        self
    }

    /// Wants the header `name: value`.
    #[must_use]
    pub fn require_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.required_headers.push((name.into(), value.into()));
        self
    }

    /// Keeps a session per client.
    #[must_use]
    pub fn stateful(mut self) -> Self {
        self.stateful = true;
        self
    }

    /// Refuses with a bare 401, no `WWW-Authenticate`.
    #[must_use]
    pub fn without_challenge(mut self) -> Self {
        self.www_authenticate = false;
        self
    }
}

/// A running tool server. It stops when it is dropped.
pub struct FakeToolServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    _stop: DropGuard,
}

impl FakeToolServer {
    /// Starts a server on a free port of the loopback address.
    pub async fn spawn(options: FakeToolServerOptions) -> Self {
        let shared = Arc::new(Shared {
            bearer: options.bearer,
            required_headers: options.required_headers,
            www_authenticate: options.www_authenticate,
            seen: Mutex::new(Vec::new()),
        });
        let stop = CancellationToken::new();
        let handler = {
            let shared = Arc::clone(&shared);
            move || Ok(Handler(Arc::clone(&shared)))
        };
        let service_stop = stop.child_token();
        let router = if options.stateful {
            let config =
                StreamableHttpServerConfig::default().with_cancellation_token(service_stop);
            Router::new().route_service(
                PATH,
                StreamableHttpService::new(
                    handler,
                    Arc::new(LocalSessionManager::default()),
                    config,
                ),
            )
        } else {
            let config = StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_json_response(true)
                .with_cancellation_token(service_stop);
            Router::new().route_service(
                PATH,
                StreamableHttpService::new(
                    handler,
                    Arc::new(NeverSessionManager::default()),
                    config,
                ),
            )
        }
        .layer(from_fn_with_state(Arc::clone(&shared), require_credentials));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = stop.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await;
        });
        FakeToolServer {
            addr,
            shared,
            _stop: stop.drop_guard(),
        }
    }

    /// The server's MCP endpoint.
    pub fn url(&self) -> String {
        format!("http://{}{PATH}", self.addr)
    }

    /// Every MCP request the server served, oldest first. A request refused for its credentials is
    /// not in it.
    pub fn seen(&self) -> Vec<SeenRequest> {
        self.shared
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The URL of an endpoint where nothing listens: a port that was free a moment ago.
    pub async fn closed_url() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}{PATH}")
    }
}

/// The credentials a request must carry, checked before MCP is spoken.
async fn require_credentials(
    State(shared): State<Arc<Shared>>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let bearer_ok = shared.bearer.as_ref().is_none_or(|want| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|got| got == format!("Bearer {want}"))
    });
    let headers_ok = shared.required_headers.iter().all(|(name, want)| {
        headers
            .get(name.as_str())
            .and_then(|v| v.to_str().ok())
            .is_some_and(|got| got == want)
    });
    if bearer_ok && headers_ok {
        return next.run(request).await;
    }
    let mut refused = (StatusCode::UNAUTHORIZED, "credentials refused").into_response();
    if shared.www_authenticate {
        refused.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            "Bearer realm=\"fake\"".parse().unwrap(),
        );
    }
    refused
}

struct Handler(Arc<Shared>);

impl Handler {
    /// Writes the request into the journal.
    fn record(
        &self,
        context: &RequestContext<RoleServer>,
        method: &str,
        call: Option<&CallToolRequestParams>,
    ) {
        let headers: Vec<(String, String)> = context
            .extensions
            .get::<axum::http::request::Parts>()
            .map(|parts| {
                parts
                    .headers
                    .iter()
                    .map(|(n, v)| {
                        (
                            n.as_str().to_ascii_lowercase(),
                            String::from_utf8_lossy(v.as_bytes()).into_owned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let bearer = headers
            .iter()
            .find(|(n, _)| n == "authorization")
            .and_then(|(_, v)| v.strip_prefix("Bearer "))
            .map(str::to_owned);
        // What the client itself puts in `_meta` (a progress token, the protocol's own keys) is not
        // what a caller asked to send.
        let meta: Map<String, Value> = context
            .meta
            .0
            .0
            .iter()
            .filter(|(k, _)| {
                k.as_str() != "progressToken" && !k.starts_with("io.modelcontextprotocol/")
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.0
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(SeenRequest {
                method: method.to_owned(),
                name: call.map(|c| c.name.to_string()),
                arguments: call.and_then(|c| c.arguments.clone()),
                meta: (call.is_some() && !meta.is_empty()).then_some(meta),
                bearer,
                headers,
            });
    }
}

fn schema(properties: Value) -> JsonObject {
    let Value::Object(map) = json!({"type": "object", "properties": properties}) else {
        unreachable!("an object")
    };
    map
}

fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "echo",
            "Answers its arguments.",
            schema(json!({"text": {"type": "string"}})),
        ),
        Tool::new("fail", "Always fails.", schema(json!({}))),
        Tool::new("slow", "Never answers.", schema(json!({}))),
        Tool::new(
            "big",
            "Answers `bytes` bytes of text.",
            schema(json!({"bytes": {"type": "integer"}})),
        ),
    ]
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.record(&context, "tools/list", None);
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.record(&context, "tools/call", Some(&request));
        let arguments = request.arguments.clone().unwrap_or_default();
        let result = match request.name.as_ref() {
            "echo" => {
                let arguments = Value::Object(arguments);
                let mut result =
                    CallToolResult::success(vec![ContentBlock::text(arguments.to_string())]);
                result.structured_content = Some(arguments);
                result
            }
            "fail" => CallToolResult::error(vec![ContentBlock::text(FAIL_TEXT)]),
            "slow" => {
                tokio::select! {
                    () = context.ct.cancelled() => {
                        return Err(ErrorData::internal_error("the caller went away", None));
                    }
                    () = tokio::time::sleep(SLOW_FOR) => {}
                }
                CallToolResult::success(vec![ContentBlock::text("done at last")])
            }
            "big" => {
                let bytes = arguments
                    .get("bytes")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0);
                CallToolResult::success(vec![ContentBlock::text("x".repeat(bytes))])
            }
            other => {
                return Err(ErrorData::invalid_params(
                    format!("unknown tool {other:?}"),
                    None,
                ));
            }
        };
        Ok(CallToolResponse::from(result))
    }
}
