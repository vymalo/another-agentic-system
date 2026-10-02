//! The MCP client against a real MCP server over HTTP (`FakeToolServer`): the `ToolServerClient`
//! conformance suite, once against a server that keeps no session and once against one that does,
//! and what only this client has to say (how a server's failures map to errors, the credentials on
//! the wire, redirects, an endpoint it cannot use).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::routing::{any, post};
use orch_ports::testkit::tool_server::{SeenRequest, ToolServerFixture};
use orch_ports::{ToolCall, ToolSecret, ToolServerClient, ToolServerEndpoint, ToolServerError};
use orch_testsupport::{FakeToolServer, FakeToolServerOptions};
use orch_tools_mcp::McpToolClient;
use serde_json::json;

const BEARER: &str = "bearer-0123456789-very-secret";
const HEADER: (&str, &str) = ("X-Fixture-Key", "header-0123456789-very-secret");

struct Fixture {
    client: McpToolClient,
    server: FakeToolServer,
    unreachable: String,
}

impl ToolServerFixture for Fixture {
    type Client = McpToolClient;

    fn client(&self) -> &McpToolClient {
        &self.client
    }

    fn bearer(&self) -> &str {
        BEARER
    }

    fn header(&self) -> (&str, &str) {
        HEADER
    }

    fn endpoint(&self) -> ToolServerEndpoint {
        credentials(ToolServerEndpoint::new(
            "fixture",
            self.server.url(),
            Duration::from_secs(10),
        ))
    }

    fn unreachable_endpoint(&self) -> ToolServerEndpoint {
        credentials(ToolServerEndpoint::new(
            "nobody",
            self.unreachable.clone(),
            Duration::from_secs(10),
        ))
    }

    fn seen(&self) -> Vec<SeenRequest> {
        self.server.seen()
    }
}

fn credentials(endpoint: ToolServerEndpoint) -> ToolServerEndpoint {
    endpoint
        .with_bearer(ToolSecret::new(BEARER))
        .with_header(HEADER.0, ToolSecret::new(HEADER.1))
}

async fn fixture(options: FakeToolServerOptions) -> Fixture {
    Fixture {
        client: McpToolClient::new().unwrap(),
        server: FakeToolServer::spawn(options.bearer(BEARER).require_header(HEADER.0, HEADER.1))
            .await,
        unreachable: FakeToolServer::closed_url().await,
    }
}

mod without_a_session {
    use super::*;

    async fn make() -> Option<Fixture> {
        Some(fixture(FakeToolServerOptions::default()).await)
    }

    orch_ports::tool_server_conformance!(make);
}

mod with_a_session {
    use super::*;

    async fn make() -> Option<Fixture> {
        Some(fixture(FakeToolServerOptions::default().stateful()).await)
    }

    orch_ports::tool_server_conformance!(make);
}

mod without_a_challenge {
    use super::*;

    async fn make() -> Option<Fixture> {
        Some(fixture(FakeToolServerOptions::default().without_challenge()).await)
    }

    orch_ports::tool_server_conformance!(make);
}

/// A server that answers every request with `status`, counting them.
async fn serve_status(status: StatusCode, body: &'static str) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    let app = Router::new().route(
        "/mcp",
        any(move || {
            let counted = Arc::clone(&counted);
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                (status, [(header::CONTENT_TYPE, "text/html")], body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/mcp"), hits)
}

fn endpoint(url: String) -> ToolServerEndpoint {
    credentials(ToolServerEndpoint::new("x", url, Duration::from_secs(5)))
}

#[tokio::test]
async fn a_server_error_is_unreachable() {
    let (url, _) = serve_status(StatusCode::SERVICE_UNAVAILABLE, "down").await;
    let err = McpToolClient::new()
        .unwrap()
        .list_tools(&endpoint(url))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ToolServerError::Unreachable { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_forbidden_answer_is_unauthenticated() {
    let (url, _) = serve_status(StatusCode::FORBIDDEN, "no").await;
    let err = McpToolClient::new()
        .unwrap()
        .list_tools(&endpoint(url))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolServerError::Unauthenticated), "{err:?}");
}

#[tokio::test]
async fn an_html_page_is_a_protocol_error() {
    let (url, _) = serve_status(StatusCode::OK, "<html>not MCP</html>").await;
    let err = McpToolClient::new()
        .unwrap()
        .list_tools(&endpoint(url))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolServerError::Protocol { .. }), "{err:?}");
}

#[tokio::test]
async fn a_server_that_accepts_and_says_nothing_is_unreachable_within_the_limit() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // It accepts connections and holds them: no answer to the handshake.
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let started = std::time::Instant::now();
    let err = McpToolClient::new()
        .unwrap()
        .list_tools(
            &endpoint(format!("http://{addr}/mcp")).with_timeout(Duration::from_millis(500)),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, ToolServerError::Unreachable { .. }),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(500) + Duration::from_secs(1));
}

#[tokio::test]
async fn a_redirect_is_not_followed_and_the_headers_go_nowhere_else() {
    // The target would see the bearer if the redirect were followed.
    let target_hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&target_hits);
    let target = Router::new().route(
        "/elsewhere",
        post(move || {
            let counted = Arc::clone(&counted);
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, target).await.unwrap() });
    let to = format!("http://{target_addr}/elsewhere");
    let origin = Router::new().route(
        "/mcp",
        any(move || {
            let to = to.clone();
            async move { (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, to)]) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, origin).await.unwrap() });

    let err = McpToolClient::new()
        .unwrap()
        .list_tools(&endpoint(format!("http://{addr}/mcp")))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ToolServerError::Protocol { .. } | ToolServerError::Unreachable { .. }
        ),
        "{err:?}"
    );
    assert_eq!(
        target_hits.load(Ordering::SeqCst),
        0,
        "the redirect was followed"
    );
}

#[tokio::test]
async fn an_endpoint_the_client_cannot_use_is_misconfigured_and_nothing_is_sent() {
    let client = McpToolClient::new().unwrap();
    let ok = ToolServerEndpoint::new("x", "http://127.0.0.1:1/mcp", Duration::from_secs(1));
    for bad in [
        ToolServerEndpoint {
            url: "ftp://example.com/mcp".to_owned(),
            ..ok.clone()
        },
        ToolServerEndpoint {
            url: "example.com/mcp".to_owned(),
            ..ok.clone()
        },
        ok.clone()
            .with_header("Authorization", ToolSecret::new("Bearer other")),
        ok.clone()
            .with_header("Mcp-Session-Id", ToolSecret::new("x")),
        ok.clone().with_header("bad name", ToolSecret::new("v")),
        ok.clone()
            .with_header("X-Ok", ToolSecret::new("line\nbreak")),
    ] {
        let err = client.list_tools(&bad).await.unwrap_err();
        assert!(
            matches!(err, ToolServerError::Misconfigured(_)),
            "{bad:?}: {err:?}"
        );
        let err = client
            .call_tool(&bad, &ToolCall::new("echo"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolServerError::Misconfigured(_)),
            "{bad:?}: {err:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_is_read_on_every_request() {
    // Nothing is cached between requests: each lists again.
    let f = fixture(FakeToolServerOptions::default()).await;
    let endpoint = f.endpoint();
    f.client.list_tools(&endpoint).await.unwrap();
    f.client.list_tools(&endpoint).await.unwrap();
    let lists = f
        .server
        .seen()
        .iter()
        .filter(|r| r.method == "tools/list")
        .count();
    assert_eq!(lists, 2);
}

#[tokio::test]
async fn the_arguments_and_the_meta_arrive_as_json_the_server_can_read() {
    let f = fixture(FakeToolServerOptions::default()).await;
    let call = ToolCall::new("echo")
        .with_arguments(
            json!({"text": "héllo \u{1F600}", "list": [1, 2.5, null]})
                .as_object()
                .unwrap()
                .clone(),
        )
        .with_meta(
            json!({"thread-tools/v1": {"callId": "c"}, "other": true})
                .as_object()
                .unwrap()
                .clone(),
        );
    let output = f.client.call_tool(&f.endpoint(), &call).await.unwrap();
    assert_eq!(
        output.structured,
        Some(serde_json::Value::Object(call.arguments.clone()))
    );
    let seen = f.server.seen();
    let request = seen.iter().find(|r| r.method == "tools/call").unwrap();
    assert_eq!(request.meta, call.meta);
}
