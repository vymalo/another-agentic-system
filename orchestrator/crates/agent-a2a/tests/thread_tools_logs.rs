//! The thread-tools token reaches the agent's message and no log line (`thread-tools/v1`, ADR 0023):
//! everything the adapter, the A2A SDK and the in-process agent log while a message is sent, at the
//! most verbose level, is kept and searched for the token and the key.
//!
//! One test in a file of its own: a thread-local `tracing` subscriber hears the callsites of a
//! process only when no other test of the process has registered them first, so it cannot share a
//! process with the tests that run in parallel in `thread_tools.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use jiff::Timestamp;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{AgentId, THREAD_TOOLS_EXTENSION, ThreadId, ToolsGrant};
use orch_ports::{AgentClient, SendContent, SendRequest};
use orch_testsupport::{FakeAgent, FakeAgentOptions};
use orch_thread_token::{ThreadToolsIssuer, ThreadToolsKeys};
use secrecy::SecretString;
use tracing_subscriber::fmt::MakeWriter;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn grant() -> ToolsGrant {
    ToolsGrant::main(
        ThreadId(uuid::Uuid::from_u128(
            0x0192_7a4e_3b00_7000_8000_0000_0000_0001,
        )),
        3,
        AgentId::new("coder"),
    )
}

/// A `tracing` writer that keeps what it is given.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

#[tokio::test]
async fn the_token_is_in_the_message_and_nowhere_else() {
    // Everything the adapter, the A2A SDK and the in-process agent log while a message is sent, at
    // the most verbose level, is kept (the runtime is single-threaded, so this thread-local
    // subscriber hears every task).
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    let fake = FakeAgent::spawn(FakeAgentOptions {
        extensions: vec![THREAD_TOOLS_EXTENSION.to_owned()],
        ..FakeAgentOptions::default()
    })
    .await;
    let issuer = Arc::new(
        ThreadToolsIssuer::new(
            ThreadToolsKeys::new(SecretString::from(KEY.to_owned()), None).unwrap(),
            "http://orchestrator:8080",
            Duration::from_secs(7200),
        )
        .unwrap(),
    );
    let client = A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        thread_tools: Some(Arc::clone(&issuer)),
        ..A2aConfig::default()
    })
    .unwrap();
    let req = SendRequest {
        endpoint: fake.endpoint("coder", None),
        message_id: "msg-1".to_owned(),
        context_id: "ctx-1".to_owned(),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("echo hi".to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: Some(grant()),
        history: None,
        steer: false,
        mentions: Vec::new(),
    };
    let shown_request = format!("{req:?}");
    let mut stream = client.send_stream(req).await.unwrap();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(20), stream.next())
        .await
        .unwrap()
    {
        item.unwrap();
    }
    let call = fake.executions().pop().unwrap();
    let token = call.thread_tools.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(guard);

    // What the request holds is the grant, which is no secret; the token is not in it.
    assert!(shown_request.contains("thread_tools"), "{shown_request}");
    assert!(!shown_request.contains(&token), "{shown_request}");
    // What the adapter and its configuration print never shows a token or a key.
    for shown in [
        format!("{client:?}"),
        format!("{issuer:?}"),
        format!("{:?}", issuer.keys()),
        format!(
            "{:?}",
            issuer.grant(&grant(), "msg-x", Timestamp::now()).unwrap()
        ),
    ] {
        assert!(!shown.contains(&token), "{shown}");
        assert!(!shown.contains(KEY), "{shown}");
    }
    // And no log line of the whole send holds the token or the key.
    let log = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
    assert!(
        log.len() > 100,
        "the capture heard nothing, so it proves nothing: {log:?}"
    );
    assert!(!log.contains(&token), "the token reached a log line");
    assert!(!log.contains(KEY), "the key reached a log line");
    // nor does a segment of it alone (the header and the claims are only base64)
    for segment in token.split('.') {
        assert!(
            !log.contains(segment),
            "a segment of the token reached a log line"
        );
    }
}
