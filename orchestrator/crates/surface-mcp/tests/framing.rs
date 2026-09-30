//! How a tool's answer is framed on the wire: one `application/json` response written with its head,
//! not a `text/event-stream` whose head goes out before the tool has answered.
//!
//! The reason is a proxy that drops the upstream connection as soon as it has the response head. A
//! Go `net/http` reverse proxy (Caddy, the dev edge; whether oauth2-proxy does too is unverified)
//! does exactly that when its own server closes the client's request body on writing the response head while the
//! transport has not yet finished reading that body to its end: the transport takes the read error
//! for a failed request write and closes the connection. Whatever the upstream writes after the head
//! is then lost, and with a stream opened before the answer that was the whole answer (coder-e2e,
//! `dev/mcp-e2e.sh`: `FAIL start_job: {}`, and rmcp's "failed to send pending response during
//! drain"). With the answer in the same write as the head, what the proxy has already read is the
//! whole response.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn start_job(request_id: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "start_job", "arguments": {
            "text": text, "client_request_id": request_id,
        }}
    })
}

/// Plays the proxy: sends `body` as one `POST /mcp` on a fresh connection, reads until the response
/// head is complete, then drops the connection, keeping only the bytes that had arrived by then.
async fn post_then_drop_after_head(h: &Harness, body: &Value) -> String {
    let addr = h.base.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nAuthorization: Bearer {ALICE_TOKEN}\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut received = Vec::new();
    let mut buf = [0_u8; 16 * 1024];
    while !received.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = tokio::time::timeout(T, stream.read(&mut buf))
            .await
            .expect("a response head")
            .unwrap();
        assert!(n > 0, "the server closed before the response head");
        received.extend_from_slice(&buf[..n]);
    }
    drop(stream);
    String::from_utf8(received).unwrap()
}

/// The JSON-RPC answer to request `id` in `response` (raw HTTP/1.1, a JSON body or `data:` lines),
/// if all of it is there.
fn answer(response: &str, id: i64) -> Option<Value> {
    let (_, body) = response.split_once("\r\n\r\n")?;
    let complete = |v: &Value| v["id"] == id;
    serde_json::from_str::<Value>(body)
        .ok()
        .filter(complete)
        .or_else(|| {
            body.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
                .find(complete)
        })
}

#[tokio::test]
async fn a_proxy_that_drops_the_connection_once_it_has_the_head_still_has_the_answer() {
    let h = Harness::start_with(Options {
        heartbeat: Some(Duration::from_millis(200)),
        ..Options::default()
    })
    .await;
    // A tool that takes a while to answer, as `start_job` does on Postgres: a wait on a job whose
    // agent is held, without a progress token, answers `timed_out` after one heartbeat.
    let (name, value) = bearer(ALICE_TOKEN);
    let started = messages(
        h.post(&[(name, &value)], &start_job("framing-held", "gate held"))
            .await,
    )
    .await;
    let job = started[0]["result"]["structuredContent"]["job_id"]
        .as_str()
        .expect("a job id")
        .to_owned();
    let wait = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "wait_for_job", "arguments": {"job_id": job, "timeout_secs": 60}}
    });
    let response = post_then_drop_after_head(&h, &wait).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let got = answer(&response, 7)
        .unwrap_or_else(|| panic!("the head arrived without the answer:\n{response}"));
    assert_eq!(
        got["result"]["structuredContent"]["outcome"], "timed_out",
        "{got}"
    );

    // And `start_job` itself, which is the call that failed in coder-e2e.
    for n in 0..10 {
        let response =
            post_then_drop_after_head(&h, &start_job(&format!("framing-{n}"), "echo do it")).await;
        let got = answer(&response, 7).unwrap_or_else(|| {
            panic!("call {n}: the head arrived without the answer:\n{response}")
        });
        assert!(
            got["result"]["structuredContent"]["job_id"].is_string(),
            "{got}"
        );
    }
    h.agent.release_gate();
}

#[tokio::test]
async fn a_tool_answer_is_one_json_response_with_a_length() {
    let h = Harness::start().await;
    let calls = [
        start_job("framing-json", "echo do it"),
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
               "params": {"name": "list_agents", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {}}),
    ];
    for call in calls {
        let resp = h
            .post(&[(bearer(ALICE_TOKEN).0, &bearer(ALICE_TOKEN).1)], &call)
            .await;
        assert_eq!(resp.status(), 200);
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        assert_eq!(
            header("content-type").as_deref(),
            Some("application/json"),
            "{call}"
        );
        assert!(header("content-length").is_some(), "{call}");
        let messages = messages(resp).await;
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0]["id"], 7, "{messages:?}");
        assert!(messages[0].get("result").is_some(), "{messages:?}");
    }
}

#[tokio::test]
async fn a_wait_with_a_progress_token_is_still_an_event_stream() {
    let h = Harness::start().await;
    let started = messages(
        h.post(
            &[(bearer(ALICE_TOKEN).0, &bearer(ALICE_TOKEN).1)],
            &start_job("framing-wait", "echo do it"),
        )
        .await,
    )
    .await;
    let job = started[0]["result"]["structuredContent"]["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let wait = tool_call(
        "wait_for_job",
        json!({"job_id": job, "after_seq": 0, "timeout_secs": 5}),
        Some("p1"),
    );
    let resp = h
        .post(&[(bearer(ALICE_TOKEN).0, &bearer(ALICE_TOKEN).1)], &wait)
        .await;
    assert_eq!(resp.status(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        content_type.starts_with("text/event-stream"),
        "{content_type}"
    );
    let messages = messages(resp).await;
    assert!(
        messages
            .iter()
            .any(|m| m["method"] == "notifications/progress"),
        "{messages:?}"
    );
    assert_eq!(messages.last().unwrap()["id"], 1, "{messages:?}");
}
