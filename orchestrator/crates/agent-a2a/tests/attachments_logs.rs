//! The signed link of a message's file reaches the agent's message and no log line
//! (`docs/api/attachments.md`, "The message side"): everything the adapter, the A2A SDK, the HTTP
//! stack and the in-process agent log while the message is sent, at the most verbose level, is kept
//! and searched for the link; and so for the file's name.
//!
//! One test in a file of its own, for the reason `thread_tools_logs.rs` gives.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::ThreadId;
use orch_ports::testkit::agent_client::{LINK_SECRET, TestLinks, test_link};
use orch_ports::{AgentClient, ArtifactMeta, SendAttachment, SendContent, SendRequest};
use orch_testsupport::{FakeAgent, FakeAgentOptions};
use tracing_subscriber::fmt::MakeWriter;

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
async fn the_link_is_in_the_message_and_in_no_log_line() {
    // the runtime is single-threaded, so this thread-local subscriber hears every task
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let client = A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        attachment_links: Some(Arc::new(TestLinks)),
        ..A2aConfig::default()
    })
    .unwrap();
    const NAME: &str = "pumpernickel-payslip.pdf";
    let file = SendAttachment {
        key: ArtifactMeta::of("application/pdf", None, b"x")
            .key(ThreadId(uuid::Uuid::from_u128(1))),
        media_type: "application/pdf".to_owned(),
        filename: Some(NAME.to_owned()),
    };
    let req = SendRequest {
        endpoint: fake.endpoint("fake", None),
        message_id: "msg-1".to_owned(),
        context_id: Some("ctx-1".to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("echo hi".to_owned()),
        attachments: vec![file.clone()],
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
        signed_prompt: None,
    };
    let shown = format!("{req:?}");
    // the request as a log line would have it
    tracing::debug!(request = ?req, "canary-request");
    let mut stream = client.send_stream(req).await.unwrap();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(20), stream.next())
        .await
        .unwrap()
    {
        item.unwrap();
    }
    drop(guard);

    // the agent did get the link, so there was something to leak
    let call = fake.executions().pop().unwrap();
    assert_eq!(call.parts[1]["url"], test_link(&file.key));

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    // and the capture did hear this thread
    assert!(logs.contains("canary-request"), "{logs}");
    for text in [&logs, &shown] {
        for secret in [LINK_SECRET, "files.test.invalid", "X-Amz-Signature", NAME] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
    }
}
