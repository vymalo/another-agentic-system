//! One scenario for the tests that search the logs for what a stranger's MCP server was lent
//! (ADR 0046, slice 5a review): the person's server says the credential back in every part of a
//! session, fails the stream and the end of the session with a 500, and sends a message before the
//! handshake; the agent lists its tools and calls one; and everything the process logged is
//! returned, captured under the filter the **binary** builds for a given `RUST_LOG`
//! (`orch_log_filter`, the function `logging::init` calls).
//!
//! Each test file that uses it is a process of its own: `tracing` caches which call sites are
//! wanted, and a subscriber of one thread misses a site another thread met first.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::{Arc, Mutex};

use orch_ports::EgressPolicy;
use orch_testsupport::{HostileOptions, HostileToolServer};
use orch_tools_mcp::McpToolClient;
use serde_json::json;
use tracing_subscriber::fmt::format::FmtSpan;

use super::call;
use super::wire::WireRig;

/// The credential the person typed.
pub const BEARER: &str = "SENTINEL-bearer-7d41c09e5b";
/// In the path of the person's URL, as a webhook-style URL has a secret.
pub const IN_PATH: &str = "SENTINEL-in-the-path-a83f6e1d02";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// What the process logged while the scenario ran, and what the hostile server was asked.
pub struct Run {
    pub logs: String,
    pub asked: Vec<String>,
}

/// Runs the scenario with the service's log filter for `rust_log`.
pub async fn run(rust_log: Option<&str>) -> Run {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(orch_log_filter::env_filter(rust_log))
        .with_span_events(FmtSpan::FULL)
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    // a canary: the capture finds a secret if one is logged
    tracing::info!(canary = BEARER, "the canary");

    let hostile = HostileToolServer::spawn(
        HostileOptions::default()
            .bearer(BEARER)
            .at_path(format!("/{IN_PATH}/mcp")),
    )
    .await;
    let policy = EgressPolicy::new()
        .allowing_http()
        .with_allow("127.0.0.1".parse().unwrap());
    let rig = WireRig::start(policy.clone(), McpToolClient::guarded(policy).unwrap()).await;
    let id = rig
        .add(json!({"name": "Hostile", "url": hostile.url(),
                    "auth": {"kind": "bearer"}, "secret": BEARER}))
        .await;
    let thread = rig.working_thread(&[&id]).await;
    let client = rig.client(thread).await;

    // the agent lists the tools and calls one, twice, so the session is opened and closed again
    let tools = client.list_all_tools().await.unwrap();
    assert!(
        tools.iter().any(|t| t.name.ends_with("__echo")),
        "the hostile server's tool is offered: {tools:?}"
    );
    for text in ["one", "two"] {
        let out = call(&client, &format!("{id}__echo"), json!({"text": text})).await;
        assert!(!out.is_error, "{out:?}");
        // what the stranger said back is scrubbed of the credential, which is the relay's half
        assert!(!out.text.contains(BEARER), "{out:?}");
    }
    // the person's views, and the end of the thread
    let (status, text) = rig
        .api(reqwest::Method::GET, "/api/user-tool-servers", None)
        .await;
    assert_eq!(status, 200, "{text}");
    rig.app.cancel(&WireRig::alice(), thread).await.unwrap();
    orch_testsupport::eventually("the thread to end", || async {
        let record = rig.app.get_thread(&WireRig::alice(), thread).await.unwrap();
        record.state.is_terminal().then_some(())
    })
    .await;

    let asked = hostile.seen();
    // the session really went through the parts that make the library log: a stream and an end
    assert!(
        asked.iter().any(|a| a == "POST initialize")
            && asked.iter().any(|a| a == "POST tools/call"),
        "{asked:?}"
    );
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("the canary"),
        "the capture does not see what is logged"
    );
    Run { logs, asked }
}

/// Every line of `logs` that holds one of the secrets of the scenario.
pub fn leaks(logs: &str) -> Vec<&str> {
    logs.lines()
        .filter(|line| line.contains(BEARER) || line.contains(IN_PATH))
        // the canary is the capture's own control
        .filter(|line| !line.contains("the canary"))
        .collect()
}
