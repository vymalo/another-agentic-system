//! The first task of a fork is told the conversation it continues (ADR 0029): the local-agent
//! client puts it in front of the message, in the same text, as the A2A client does.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use futures::StreamExt as _;
use orch_agent_adam::testkit::{LocalFixture, scripted_endpoint};
use orch_core::{AgentUpdate, ForkHistory, HistoryEntry, HistoryRole, history_preamble};
use orch_ports::{AgentClient as _, SendContent, SendRequest};

fn request(text: &str, message_id: &str, history: Option<ForkHistory>) -> SendRequest {
    SendRequest {
        endpoint: scripted_endpoint("scripted"),
        message_id: message_id.to_owned(),
        context_id: Some("ctx-fork".to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(text.to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history,
        steer: false,
        mentions: Vec::new(),
    }
}

/// The text of the artifact the scripted agent answered with: `echo: <the whole message>`.
async fn echoed(fixture: &LocalFixture, req: SendRequest) -> String {
    let mut stream = fixture.instance().client.send_stream(req).await.unwrap();
    let mut said = None;
    while let Some(env) = stream.next().await {
        if let Some(AgentUpdate::Artifact { text, .. }) = env.unwrap().update {
            said = text;
        }
    }
    said.expect("an artifact")
}

#[tokio::test]
async fn the_conversation_of_a_fork_comes_before_the_message() {
    let fixture = LocalFixture::memory().await;
    let history = ForkHistory {
        entries: vec![HistoryEntry {
            role: HistoryRole::Person,
            name: "person".to_owned(),
            text: "fix the redirect loop".to_owned(),
        }],
        omitted: 0,
    };
    let told = echoed(&fixture, request("go on", "m-1", Some(history.clone()))).await;
    assert_eq!(
        told,
        format!("echo: {}go on", history_preamble(&history)),
        "the same text part: the conversation, a blank line, the message"
    );
    let plain = echoed(&fixture, request("go on", "m-2", None)).await;
    assert_eq!(plain, "echo: go on");
}
