//! The client of the servers people name against a server that tries to make the orchestrator
//! buffer what it likes (ADR 0046, slice 5a review): an answer that never ends, a listing with no
//! last page, a tool with a schema the size of a book, a name that never resolves.
//!
//! The assertions are on the **cut**, not on memory: the client fails with "larger than allowed",
//! and the server sees the connection closed (its body stream is dropped) after it produced no more
//! than the bound and what the kernel's buffers hold.
//!
//! The server is not rmcp's: it speaks just enough JSON-RPC to get a client through `initialize`
//! and then answers what each test scripts.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use orch_ports::{
    EgressPolicy, MAX_USER_TOOL_SCHEMA_BYTES, ToolCall, ToolServerClient, ToolServerEndpoint,
    ToolServerError,
};
use orch_tools_mcp::{GuardedLimits, HostLookup, LookupFuture, McpToolClient};
use serde_json::{Value, json};

const TOO_LARGE: &str = "the server's answer is larger than allowed";

/// What the server answers to one request.
#[derive(Clone)]
enum Reply {
    /// One JSON-RPC result, in a plain JSON body.
    Result(Value),
    /// The start of a JSON-RPC result that never ends, in a chunked JSON body.
    EndlessJson,
    /// An event stream of small events that never ends and never answers.
    EndlessSse,
    /// A 500 whose body is JSON that never ends: an error message the size of the internet.
    EndlessError,
    /// A status and a short body.
    Status(StatusCode),
}

type Script = dyn Fn(&str, &Value) -> Reply + Send + Sync;

struct Shared {
    script: Box<Script>,
    /// The `method` of every request, in order.
    requests: Mutex<Vec<String>>,
    /// Bytes handed to the socket by endless bodies.
    produced: AtomicUsize,
    /// Set when an endless body is dropped: the connection was closed under it.
    cut: AtomicBool,
}

struct Hostile {
    url: String,
    shared: Arc<Shared>,
}

impl Hostile {
    /// A server that answers `initialize` as MCP does and everything else as `script` says.
    async fn spawn(script: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static) -> Self {
        Self::spawn_at("/mcp", script).await
    }

    /// [`spawn`](Self::spawn), mounted at `path`.
    async fn spawn_at(
        path: &str,
        script: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            script: Box::new(script),
            requests: Mutex::new(Vec::new()),
            produced: AtomicUsize::new(0),
            cut: AtomicBool::new(false),
        });
        let router = Router::new()
            .route(path, post(handle))
            .with_state(Arc::clone(&shared));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Hostile {
            url: format!("http://{addr}{path}"),
            shared,
        }
    }

    fn requests(&self, method: &str) -> usize {
        self.shared
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|m| *m == method)
            .count()
    }

    async fn saw_the_connection_closed(&self) -> bool {
        for _ in 0..100 {
            if self.shared.cut.load(Ordering::SeqCst) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }
}

/// What an endless body holds: the counters, and a flag set when the body is dropped.
struct Endless {
    shared: Arc<Shared>,
    chunk: Bytes,
    first: Option<Bytes>,
}

impl Drop for Endless {
    fn drop(&mut self) {
        self.shared.cut.store(true, Ordering::SeqCst);
    }
}

fn endless(shared: &Arc<Shared>, first: &'static str, chunk: Bytes) -> Body {
    let state = Endless {
        shared: Arc::clone(shared),
        chunk,
        first: Some(Bytes::from_static(first.as_bytes())),
    };
    Body::from_stream(futures::stream::unfold(state, |mut state| async move {
        let next = state.first.take().unwrap_or_else(|| state.chunk.clone());
        state
            .shared
            .produced
            .fetch_add(next.len(), Ordering::SeqCst);
        Some((Ok::<_, io::Error>(next), state))
    }))
}

async fn handle(State(shared): State<Arc<Shared>>, body: Bytes) -> Response {
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let method = request["method"].as_str().unwrap_or_default().to_owned();
    let id = request["id"].clone();
    shared.requests.lock().unwrap().push(method.clone());
    let reply = match method.as_str() {
        "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
        "initialize" => {
            let scripted = (shared.script)(&method, &request["params"]);
            // `Result(Null)` from a script means "answer the handshake as a server does"
            match scripted {
                Reply::Result(Value::Null) => Reply::Result(json!({
                    "protocolVersion": request["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "hostile", "version": "1"},
                })),
                other => other,
            }
        }
        other => (shared.script)(other, &request["params"]),
    };
    match reply {
        Reply::Result(result) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        )
            .into_response(),
        Reply::EndlessJson => {
            let prefix = r#"{"jsonrpc":"2.0","id":1,"result":{"padding":""#;
            let mut response =
                Response::new(endless(&shared, prefix, Bytes::from(vec![b'x'; 16 * 1024])));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }
        Reply::EndlessSse => {
            // small events: the per-event limit never trips, only the total can stop it
            let event = Bytes::from(
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{\"level\":\"info\",\"data\":\"tick\"}}\n\n"
                    .repeat(64),
            );
            let mut response = Response::new(endless(&shared, "", event));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
        }
        Reply::Status(status) => (status, "a short body").into_response(),
        Reply::EndlessError => {
            let mut response = Response::new(endless(
                &shared,
                r#"{"error":""#,
                Bytes::from(vec![b'x'; 16 * 1024]),
            ));
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }
    }
}

fn endpoint(url: &str) -> ToolServerEndpoint {
    ToolServerEndpoint::new("hostile", url, Duration::from_secs(20))
}

fn admitting_loopback() -> EgressPolicy {
    EgressPolicy::new()
        .allowing_http()
        .with_allow("127.0.0.1".parse().unwrap())
}

fn guarded(limits: GuardedLimits) -> McpToolClient {
    McpToolClient::guarded_with_limits(admitting_loopback(), orch_tools_mcp::SystemLookup, limits)
        .unwrap()
}

/// 256 KiB: far above any ordinary answer of these tests, far below what an endless body produces
/// before the kernel stops it.
const CAP: usize = 256 * 1024;

fn small_cap() -> GuardedLimits {
    GuardedLimits::default().with_max_response_bytes(CAP)
}

/// The most an endless body can have produced by the time the client's close is seen: the bound,
/// and what the kernel buffers on both ends (loopback autotuning stays under 16 MiB).
const SLACK: usize = CAP + 16 * 1024 * 1024;

async fn assert_cut(server: &Hostile, error: ToolServerError) {
    assert!(
        matches!(&error, ToolServerError::Protocol { detail, .. } if detail == TOO_LARGE),
        "{error:?}"
    );
    assert!(
        server.saw_the_connection_closed().await,
        "the server's body was never dropped: the connection stayed open"
    );
    let produced = server.shared.produced.load(Ordering::SeqCst);
    assert!(produced > CAP, "the bound was never reached: {produced}");
    assert!(produced < SLACK, "the server produced {produced} bytes");
}

// ---- an answer that never ends is cut on the wire ----------------------------------------------------

#[tokio::test]
async fn a_json_answer_that_never_ends_is_cut_at_the_bound() {
    // the handshake itself never ends
    let server = Hostile::spawn(|_, _| Reply::EndlessJson).await;
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        guarded(small_cap()).list_tools(&endpoint(&server.url)),
    )
    .await
    .expect("the client must stop reading at the bound, not at the timeout")
    .unwrap_err();
    assert_cut(&server, error).await;
}

#[tokio::test]
async fn an_event_stream_of_small_events_that_never_ends_is_cut_at_the_bound() {
    // no single event is large, so the per-event limit of rmcp never trips
    let server = Hostile::spawn(|_, _| Reply::EndlessSse).await;
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        guarded(small_cap()).list_tools(&endpoint(&server.url)),
    )
    .await
    .expect("the client must stop reading at the bound, not at the timeout")
    .unwrap_err();
    assert_cut(&server, error).await;
}

#[tokio::test]
async fn a_listing_that_never_ends_is_cut_at_the_bound() {
    // a good handshake, then a `tools/list` that is a JSON body without an end
    let server = Hostile::spawn(|method, _| match method {
        "initialize" => Reply::Result(Value::Null),
        _ => Reply::EndlessJson,
    })
    .await;
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        guarded(small_cap()).list_tools(&endpoint(&server.url)),
    )
    .await
    .expect("the client must stop reading at the bound, not at the timeout")
    .unwrap_err();
    assert_cut(&server, error).await;
}

#[tokio::test]
async fn a_call_whose_stream_never_ends_is_cut_at_the_bound() {
    let server = Hostile::spawn(|method, _| match method {
        "initialize" => Reply::Result(Value::Null),
        _ => Reply::EndlessSse,
    })
    .await;
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        guarded(small_cap()).call_tool(&endpoint(&server.url), &ToolCall::new("anything")),
    )
    .await
    .expect("the client must stop reading at the bound, not at the timeout")
    .unwrap_err();
    assert_cut(&server, error).await;
}

#[tokio::test]
async fn an_error_whose_body_never_ends_is_a_500_and_the_body_is_not_read_to_its_end() {
    // rmcp reads an error body whole (`response.text()`): this client reads a bounded part and
    // says what the status said
    let server = Hostile::spawn(|_, _| Reply::EndlessError).await;
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        guarded(small_cap()).list_tools(&endpoint(&server.url)),
    )
    .await
    .expect("the client must stop reading at the bound, not at the timeout")
    .unwrap_err();
    assert!(
        matches!(error, ToolServerError::Unreachable { .. }),
        "{error:?}"
    );
    assert!(server.saw_the_connection_closed().await);
    assert!(server.shared.produced.load(Ordering::SeqCst) < SLACK);
}

#[tokio::test]
async fn an_answer_within_the_bound_is_read_whole() {
    // 100 KiB of tools is under the 256 KiB bound
    let tools: Vec<Value> = (0..100)
        .map(|n| {
            json!({"name": format!("tool-{n}"), "description": "d".repeat(900),
                        "inputSchema": {"type": "object"}})
        })
        .collect();
    let server = Hostile::spawn(move |method, _| match method {
        "initialize" => Reply::Result(Value::Null),
        _ => Reply::Result(json!({"tools": tools})),
    })
    .await;
    let listed = guarded(small_cap())
        .list_tools(&endpoint(&server.url))
        .await
        .unwrap();
    assert_eq!(listed.len(), 100);
}

// ---- a listing with no last page, a schema the size of a book ---------------------------------------

#[tokio::test]
async fn a_listing_with_no_last_page_stops_at_the_page_cap() {
    // every page names a next one: only the cap ends it
    let server = Hostile::spawn(|method, params| match method {
        "initialize" => Reply::Result(Value::Null),
        _ => {
            let n: usize = params["cursor"]
                .as_str()
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            Reply::Result(json!({
                "tools": [{"name": format!("tool-{n}"), "inputSchema": {"type": "object"}}],
                "nextCursor": (n + 1).to_string(),
            }))
        }
    })
    .await;
    let limits = GuardedLimits::default().with_max_pages(4);
    let listed = guarded(limits)
        .list_tools(&endpoint(&server.url))
        .await
        .unwrap();
    assert_eq!(listed.len(), 4, "what four pages hold");
    assert_eq!(server.requests("tools/list"), 4, "no fifth page was asked");
}

#[tokio::test]
async fn a_listing_is_cut_at_the_tool_cap_in_the_middle_of_a_page() {
    let server = Hostile::spawn(|method, _| match method {
        "initialize" => Reply::Result(Value::Null),
        _ => {
            let tools: Vec<Value> = (0..5)
                .map(|n| json!({"name": format!("tool-{n}"), "inputSchema": {"type": "object"}}))
                .collect();
            Reply::Result(json!({"tools": tools, "nextCursor": "more"}))
        }
    })
    .await;
    let limits = GuardedLimits::default().with_max_tools(3);
    let listed = guarded(limits)
        .list_tools(&endpoint(&server.url))
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["tool-0", "tool-1", "tool-2"]
    );
    assert_eq!(server.requests("tools/list"), 1, "no page after the cap");
}

#[tokio::test]
async fn the_deployments_client_still_follows_every_page() {
    // the same endless-paging server, finite here: 25 pages, more than the cap of a stranger's
    let server = Hostile::spawn(|method, params| match method {
        "initialize" => Reply::Result(Value::Null),
        _ => {
            let n: usize = params["cursor"]
                .as_str()
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            let mut page = json!({
                "tools": [{"name": format!("tool-{n}"), "inputSchema": {"type": "object"}}],
            });
            if n < 24 {
                page["nextCursor"] = json!((n + 1).to_string());
            }
            Reply::Result(page)
        }
    })
    .await;
    let listed = McpToolClient::new()
        .unwrap()
        .list_tools(&endpoint(&server.url))
        .await
        .unwrap();
    assert_eq!(listed.len(), 25);
}

#[tokio::test]
async fn a_tool_with_a_schema_over_the_limit_is_left_out_and_the_rest_are_kept() {
    let big = "x".repeat(MAX_USER_TOOL_SCHEMA_BYTES + 1);
    let server = Hostile::spawn(move |method, _| match method {
        "initialize" => Reply::Result(Value::Null),
        _ => Reply::Result(json!({"tools": [
            {"name": "fine", "inputSchema": {"type": "object"}},
            {"name": "book", "inputSchema": {"type": "object", "description": big}},
            {"name": "also-fine", "inputSchema": {"type": "object"}},
        ]})),
    })
    .await;
    let listed = guarded(GuardedLimits::default().with_max_response_bytes(1024 * 1024))
        .list_tools(&endpoint(&server.url))
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["fine", "also-fine"]
    );
}

// ---- a name that never resolves --------------------------------------------------------------------

/// A lookup that never answers.
struct Silent;

impl HostLookup for Silent {
    fn lookup<'a>(&'a self, _host: &'a str) -> LookupFuture<'a> {
        Box::pin(std::future::pending::<io::Result<Vec<IpAddr>>>())
    }
}

#[tokio::test(start_paused = true)]
async fn a_lookup_that_never_answers_is_cut_by_the_resolver_before_the_connect_timeout() {
    let limits = GuardedLimits::default().with_lookup_timeout(Duration::from_secs(1));
    let client = McpToolClient::guarded_with_limits(
        EgressPolicy::new()
            .allowing_http()
            .with_allow("silent.example".parse().unwrap()),
        Silent,
        limits,
    )
    .unwrap();
    let started = tokio::time::Instant::now();
    let error = client
        .list_tools(&endpoint("http://silent.example:9/mcp"))
        .await
        .unwrap_err();
    let waited = started.elapsed();
    assert!(
        matches!(error, ToolServerError::Unreachable { .. }),
        "{error:?}"
    );
    // The connect timeout of the client is 10 s: without the timeout in the resolver this is the
    // time it takes.
    assert!(
        waited >= Duration::from_secs(1) && waited < Duration::from_secs(5),
        "{waited:?}"
    );
}

// ---- no error holds the URL ------------------------------------------------------------------------

/// Every word an error and the errors under it say.
fn everything_said_by(error: &ToolServerError) -> String {
    let mut said = format!("{error} {error:?} {}", error.public_detail());
    let mut source = std::error::Error::source(error);
    while let Some(e) = source {
        said.push_str(&format!(" {e} {e:?}"));
        source = e.source();
    }
    said
}

#[tokio::test]
async fn no_error_of_either_client_holds_the_url_a_person_typed() {
    // the secret of a webhook-style URL sits in its path
    const IN_PATH: &str = "SENTINEL-in-the-path-91ac4e07";
    let path = format!("/{IN_PATH}/mcp");
    let statuses = [
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::NOT_FOUND,
        StatusCode::FORBIDDEN,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::OK,
    ];
    let clients = [
        ("guarded", guarded(small_cap())),
        ("deployment", McpToolClient::new().unwrap()),
    ];
    for (which, client) in &clients {
        for status in statuses {
            let server = Hostile::spawn_at(&path, move |_, _| Reply::Status(status)).await;
            let port = server
                .url
                .rsplit(':')
                .next()
                .unwrap()
                .split('/')
                .next()
                .unwrap()
                .to_owned();
            let error = client.list_tools(&endpoint(&server.url)).await.unwrap_err();
            let said = everything_said_by(&error);
            assert!(
                !said.contains(IN_PATH),
                "{which} {status}: the path is in {said}"
            );
            assert!(
                !said.contains(&format!("127.0.0.1:{port}")),
                "{which} {status}: {said}"
            );
        }
        // a port where nothing listens: the connect error is the HTTP client's own
        let closed = format!("http://127.0.0.1:{}{path}", {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        });
        let error = client.list_tools(&endpoint(&closed)).await.unwrap_err();
        let said = everything_said_by(&error);
        assert!(
            !said.contains(IN_PATH),
            "{which} (closed): the path is in {said}"
        );
        // hyper-util prints the socket address of a refused connection in the Debug of its error:
        // an address, which is the host of the URL and not the URL (no scheme, no path, no query)
        assert!(
            !said.contains("http://") && !said.contains("/mcp"),
            "{which} (closed): {said}"
        );
    }
}
