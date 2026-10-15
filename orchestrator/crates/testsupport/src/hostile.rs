//! A tool server that says the credential back where a library might log it: a mock for the tests
//! that search the logs of the MCP client for a secret (ADR 0046, slice 5a review).
//!
//! It is not rmcp's server: it speaks just enough streamable HTTP and JSON-RPC to get a client through
//! a session, and each part of that session says what it was sent:
//!
//! | Request | Answer |
//! |---|---|
//! | `POST initialize` | an event stream with a **notification before the result** (`notifications/progress`, whose `message` holds the `Authorization` it was sent) and then the result, whose `instructions` hold it too, with an `Mcp-Session-Id` |
//! | `POST notifications/initialized` | `202` |
//! | `POST tools/list` | one tool, `echo`, whose description holds the credential |
//! | `POST tools/call` | the arguments, and the credential |
//! | `GET` (the stream a client opens beside a session) | `500`, with the credential in the body |
//! | `DELETE` (the end of the session) | `500`, with the credential in the body |
//!
//! Every answer to a request with no matching `Authorization: Bearer <bearer>` is a `401`, so the
//! server also serves a person's server that wants a bearer.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio_util::sync::{CancellationToken, DropGuard};

/// The session id the server hands out.
const SESSION: &str = "hostile-session";

struct Shared {
    bearer: Option<String>,
    seen: Mutex<Vec<String>>,
}

/// How a [`HostileToolServer`] is set up.
#[derive(Debug, Clone, Default)]
pub struct HostileOptions {
    /// The bearer token the server wants (none: it wants no credential).
    pub bearer: Option<String>,
    /// Where it is mounted (default `/mcp`); starts with `/`.
    pub path: Option<String>,
}

impl HostileOptions {
    /// Wants `Authorization: Bearer <bearer>`.
    #[must_use]
    pub fn bearer(mut self, bearer: impl Into<String>) -> Self {
        self.bearer = Some(bearer.into());
        self
    }

    /// Mounts the server at `path`.
    #[must_use]
    pub fn at_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

/// A running hostile server. It stops when it is dropped.
pub struct HostileToolServer {
    addr: SocketAddr,
    path: String,
    shared: Arc<Shared>,
    _stop: DropGuard,
}

impl HostileToolServer {
    /// Starts a server on a free port of the loopback address.
    pub async fn spawn(options: HostileOptions) -> Self {
        let path = options.path.unwrap_or_else(|| "/mcp".to_owned());
        let shared = Arc::new(Shared {
            bearer: options.bearer,
            seen: Mutex::new(Vec::new()),
        });
        let router = Router::new()
            .route(&path, axum::routing::any(handle))
            .with_state(Arc::clone(&shared));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await;
        });
        HostileToolServer {
            addr,
            path,
            shared,
            _stop: stop.drop_guard(),
        }
    }

    /// The server's MCP endpoint.
    pub fn url(&self) -> String {
        format!("http://{}{}", self.addr, self.path)
    }

    /// What the server was asked, oldest first: `POST initialize`, `GET`, `DELETE`, ...
    pub fn seen(&self) -> Vec<String> {
        self.shared
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The `Authorization` of the request, as the line the server says back.
fn said_back(headers: &HeaderMap) -> String {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map_or_else(
            || "no credential".to_owned(),
            |value| format!("Authorization: {value}"),
        )
}

async fn handle(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let credentials = said_back(&parts.headers);
    let allowed = shared.bearer.as_ref().is_none_or(|want| {
        parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|got| got == format!("Bearer {want}"))
    });
    if !allowed {
        shared
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{} refused", parts.method));
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"hostile\"")],
            "credentials refused",
        )
            .into_response();
    }
    match parts.method {
        // the stream a client opens beside a session, and the end of the session: both fail, and
        // the failure says what the server was sent
        Method::GET | Method::DELETE => {
            shared
                .seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(parts.method.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("boom, and you sent {credentials}"),
            )
                .into_response()
        }
        Method::POST => {
            let bytes = axum::body::to_bytes(body, 1024 * 1024)
                .await
                .unwrap_or_else(|_| Bytes::new());
            post(&shared, &credentials, &bytes)
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

fn post(shared: &Shared, credentials: &str, body: &[u8]) -> Response {
    let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let method = request["method"].as_str().unwrap_or_default().to_owned();
    shared
        .seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(format!("POST {method}"));
    let id = request["id"].clone();
    let json = |result: Value| {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        )
            .into_response()
    };
    match method.as_str() {
        "initialize" => {
            // a notification before the result, then the result: both say what the server was sent
            let notification = json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {
                    "progressToken": "t",
                    "progress": 1,
                    "message": format!("hello, you sent {credentials}"),
                },
            });
            let result = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": request["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "hostile", "version": "1"},
                    "instructions": format!("use me; you sent {credentials}"),
                },
            });
            let mut response = Response::new(axum::body::Body::from(format!(
                "data: {notification}\n\ndata: {result}\n\n"
            )));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
                .headers_mut()
                .insert("mcp-session-id", HeaderValue::from_static(SESSION));
            response
        }
        "tools/list" => json(json!({"tools": [{
            "name": "echo",
            "description": format!("Answers its arguments (you sent {credentials})."),
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
        }]})),
        "tools/call" => json(json!({
            "content": [{"type": "text", "text": format!(
                "{} (you sent {credentials})", request["params"]["arguments"]
            )}],
        })),
        _ => StatusCode::ACCEPTED.into_response(),
    }
}
