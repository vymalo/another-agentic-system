//! The client of the servers people name ignores the proxy variables of the environment
//! (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`): a proxy connects where it likes, and the guard
//! judged the address the client would connect to, not the proxy's.
//!
//! `reqwest` reads the environment once per process, so the test runs the real thing in a child:
//! this test binary is started again with the variables pointing at a tripwire and asked to run
//! only [`the_child`], which lists the tools of a server through the guarded client. The server is
//! reached by a name the lookup of the test maps to the loopback, so a client that used the proxy
//! would send the request there (a name the proxy "resolves") and the tripwire would count it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::post;
use orch_ports::{EgressPolicy, ToolServerClient, ToolServerEndpoint};
use orch_tools_mcp::{HostLookup, LookupFuture, McpToolClient};
use serde_json::{Value, json};

const CHILD: &str = "ORCH_TEST_PROXY_CHILD";
const NAME: &str = "fake-host.example";

struct ToLoopback;

impl HostLookup for ToLoopback {
    fn lookup<'a>(&'a self, _host: &'a str) -> LookupFuture<'a> {
        Box::pin(async move { Ok::<_, io::Error>(vec![IpAddr::from([127, 0, 0, 1])]) })
    }
}

/// A server that speaks just enough MCP for a listing, and does not check the `Host` it is reached
/// by (rmcp's own server refuses a name it does not list).
async fn serve() -> u16 {
    async fn handle(body: Bytes) -> axum::response::Response {
        let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let result = match request["method"].as_str() {
            Some("initialize") => json!({
                "protocolVersion": request["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "plain", "version": "1"},
            }),
            Some("tools/list") => json!({"tools": [
                {"name": "echo", "inputSchema": {"type": "object"}},
            ]}),
            _ => return StatusCode::ACCEPTED.into_response(),
        };
        (
            [(header::CONTENT_TYPE, "application/json")],
            json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string(),
        )
            .into_response()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/mcp", post(handle)))
            .await
            .unwrap();
    });
    port
}

/// Runs only inside the child of the test below; anywhere else it passes at once.
#[tokio::test]
async fn the_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let port = serve().await;
    let client = McpToolClient::guarded_with_lookup(
        EgressPolicy::new()
            .allowing_http()
            .with_allow(NAME.parse().unwrap()),
        ToLoopback,
    )
    .unwrap();
    let tools = client
        .list_tools(&ToolServerEndpoint::new(
            "x",
            format!("http://{NAME}:{port}/mcp"),
            Duration::from_secs(5),
        ))
        .await
        .expect("the server is reached directly, not through the proxy of the environment");
    assert_eq!(tools.len(), 1);
}

#[tokio::test]
async fn the_guarded_client_does_not_use_the_proxy_of_the_environment() {
    // the proxy: counts the connections made to it and answers none
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            held.push(socket);
        }
    });

    let exe = std::env::current_exe().unwrap();
    let status = tokio::task::spawn_blocking(move || {
        let mut child = std::process::Command::new(exe);
        child
            .args(["--exact", "the_child", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy");
        for variable in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            child.env(variable, &proxy);
        }
        child.output().unwrap()
    })
    .await
    .unwrap();
    let said = String::from_utf8_lossy(&status.stdout).into_owned()
        + &String::from_utf8_lossy(&status.stderr);
    assert!(status.status.success(), "the child failed:\n{said}");
    assert!(
        said.contains("test the_child ... ok"),
        "the child did not run its test:\n{said}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the proxy was used");
}
