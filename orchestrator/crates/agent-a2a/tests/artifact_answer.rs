//! An agent that answers through an artifact (kagent), through the A2A adapter (ADR 0031,
//! amendment of 2026-10-07), against an in-process A2A agent over real HTTP: the reply is the
//! `working` statuses' words and **one unnamed text artifact**, and the task completes with no
//! message. That text is the turn's answer, said once, as a message marked `answer`: not an
//! artifact, and a poll of the finished task says the same message under the same key.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{AgentTaskState, AgentUpdate, MessagePurpose};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentStream, SendContent, SendRequest, TaskHandle,
};
use orch_testsupport::{FakeAgent, FakeAgentOptions};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

fn request(ep: &AgentEndpoint, text: &str) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: format!("msg-{}", text.replace(' ', "-")),
        // an agent that assigns its own contexts (ADR 0055)
        context_id: None,
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(text.to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
    }
}

async fn drain(mut stream: AgentStream) -> Vec<AgentEnvelope> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(item)) => out.push(item.unwrap()),
            Ok(None) => return out,
            Err(_) => panic!("the stream did not end; got {out:?}"),
        }
    }
}

fn answers(envs: &[AgentEnvelope]) -> Vec<&AgentEnvelope> {
    envs.iter()
        .filter(|e| matches!(e.update, Some(AgentUpdate::Message { .. })))
        .collect()
}

fn artifacts(envs: &[AgentEnvelope]) -> usize {
    envs.iter()
        .filter(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
        .count()
}

#[tokio::test]
async fn an_unnamed_text_artifact_is_the_answer_said_once_and_the_poll_says_it_under_the_same_key()
{
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("kagent", None);
    let c = client();
    let live = drain(
        c.send_stream(request(&ep, "artifact-answer kagent says hello"))
            .await
            .unwrap(),
    )
    .await;

    let said = answers(&live);
    assert_eq!(said.len(), 1, "{live:?}");
    let Some(AgentUpdate::Message {
        text,
        is_final,
        purpose,
        ..
    }) = &said[0].update
    else {
        unreachable!()
    };
    assert_eq!(text, "kagent says hello");
    assert!(*is_final);
    assert_eq!(*purpose, Some(MessagePurpose::Answer));
    assert_eq!(artifacts(&live), 0, "{live:?}");
    // before the completion, which has no words of its own
    let done = live.last().unwrap();
    assert_eq!(done.task_state, Some(AgentTaskState::Completed));
    assert!(matches!(
        done.update,
        Some(AgentUpdate::Status { detail: None, .. })
    ));
    assert_eq!(live[live.len() - 2].key, said[0].key);

    // the poll after a crash says the same message, under the key that collapses with the stream's
    let task = live[0].task_id.clone();
    let snap = c
        .get_task(&TaskHandle {
            endpoint: ep.clone(),
            task_id: task,
        })
        .await
        .unwrap();
    let polled = answers(&snap.envelopes);
    assert_eq!(polled.len(), 1, "{:?}", snap.envelopes);
    assert_eq!(
        (&polled[0].key, &polled[0].update),
        (&said[0].key, &said[0].update)
    );
    assert_eq!(artifacts(&snap.envelopes), 0);
}

#[tokio::test]
async fn an_agent_that_names_its_artifacts_is_read_as_it_was() {
    // `echo` completes with a named artifact `result` that holds a link: a deliverable, no answer
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("plain", None);
    let live = drain(client().send_stream(request(&ep, "echo hi")).await.unwrap()).await;
    assert!(answers(&live).is_empty(), "{live:?}");
    assert_eq!(artifacts(&live), 1);
}
