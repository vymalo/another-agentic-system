//! `steer/v1` (ADR 0036, `docs/api/steer-v1.md`) against the in-process A2A agent over real HTTP:
//! activation, what the message carries, the first event, and the refusals the contract names.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{AgentTaskState, AgentUpdate, STEER_EXTENSION, STEPS_EXTENSION};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, SendContent, SendRequest,
};
use orch_testsupport::{CallKind, FakeAgent, FakeAgentOptions, eventually};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

async fn agent(extensions: &[&str]) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        extensions: extensions.iter().map(|u| (*u).to_owned()).collect(),
        ..FakeAgentOptions::default()
    })
    .await
}

fn request(ep: &AgentEndpoint, text: &str) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: format!("msg-{}", text.replace(' ', "-")),
        context_id: "ctx-1".to_owned(),
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

fn steer(ep: &AgentEndpoint, task: &str, text: &str, message_id: &str) -> SendRequest {
    SendRequest {
        task_id: Some(task.to_owned()),
        message_id: message_id.to_owned(),
        steer: true,
        ..request(ep, text)
    }
}

async fn next(stream: &mut AgentStream) -> AgentEnvelope {
    tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("an event in time")
        .expect("an event")
        .unwrap()
}

/// Starts `steerable work` and reads until the task is working: the stream and the task's id.
async fn running(c: &A2aAgentClient, ep: &AgentEndpoint) -> (AgentStream, String) {
    let mut stream = c.send_stream(request(ep, "steerable work")).await.unwrap();
    loop {
        let env = next(&mut stream).await;
        if env.task_state == Some(AgentTaskState::Working) {
            return (stream, env.task_id);
        }
    }
}

#[tokio::test]
async fn an_activated_steer_is_read_by_the_running_task_which_answers_with_itself() {
    let fake = agent(&[STEER_EXTENSION, STEPS_EXTENSION]).await;
    let ep = fake.endpoint("steerable", None);
    let c = client();
    let (mut stream, task) = running(&c, &ep).await;

    let mut answer = c
        .send_stream(steer(&ep, &task, "you were wrong since line 1", "m-steer"))
        .await
        .unwrap();
    // the first event is the task, still working
    let first = next(&mut answer).await;
    assert_eq!(first.task_id, task);
    assert_eq!(first.task_state, Some(AgentTaskState::Working));
    drop(answer);

    // the task reads it at its next step and reports it on the stream that started it
    loop {
        let env = next(&mut stream).await;
        if let Some(AgentUpdate::Message { text, .. }) = &env.update {
            assert_eq!(text, "steered: you were wrong since line 1");
            break;
        }
    }
    let calls: Vec<_> = fake
        .calls()
        .into_iter()
        .filter(|c| c.kind == CallKind::Steer)
        .collect();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.task_id, task);
    assert_eq!(call.context_id, "ctx-1");
    assert_eq!(call.message_id.as_deref(), Some("m-steer"));
    assert!(call.reference_task_ids.is_empty(), "nothing is continued");
    assert!(
        call.activates(STEER_EXTENSION),
        "{:?}",
        call.extensions_header
    );
    assert!(call.message_extensions.iter().any(|e| e == STEER_EXTENSION));
    // a steer is not a reporting call: the task keeps reporting on the stream it has
    assert!(!call.activates_steps(), "{:?}", call.extensions_header);
    fake.release_gate();
}

#[tokio::test]
async fn the_same_message_id_is_read_once() {
    let fake = agent(&[STEER_EXTENSION]).await;
    let ep = fake.endpoint("steerable", None);
    let c = client();
    let (mut stream, task) = running(&c, &ep).await;
    for _ in 0..2 {
        let mut answer = c
            .send_stream(steer(&ep, &task, "once", "m-same"))
            .await
            .unwrap();
        assert_eq!(next(&mut answer).await.task_id, task);
    }
    fake.release_gate();
    let mut said = 0;
    while let Ok(Some(Ok(env))) = tokio::time::timeout(Duration::from_secs(10), stream.next()).await
    {
        if matches!(env.update, Some(AgentUpdate::Message { .. })) {
            said += 1;
        }
    }
    assert_eq!(said, 1, "read once");
}

#[tokio::test]
async fn an_agent_whose_card_does_not_list_the_extension_is_never_sent_the_steer() {
    let fake = agent(&[]).await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    let mut stream = c.send_stream(request(&ep, "gate work")).await.unwrap();
    let task = next(&mut stream).await.task_id;
    let err = c
        .send_stream(steer(&ep, &task, "hello", "m-steer"))
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, AgentError::Unsupported(_)), "{err:?}");
    assert!(
        fake.calls().iter().all(|c| c.kind != CallKind::Steer),
        "nothing was sent"
    );
    // a near miss is not the extension either
    fake.set_extensions(&["https://agents.vymalo.com/a2a/extensions/steer/v2"]);
    let err = c
        .send_stream(steer(&ep, &task, "hello", "m-steer"))
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, AgentError::Unsupported(_)), "{err:?}");
    fake.release_gate();
}

#[tokio::test]
async fn the_card_is_read_for_every_steer_and_never_remembered() {
    let fake = agent(&[STEER_EXTENSION]).await;
    let ep = fake.endpoint("steerable", None);
    let c = client();
    let (_stream, task) = running(&c, &ep).await;
    let mut answer = c
        .send_stream(steer(&ep, &task, "one", "m-1"))
        .await
        .unwrap();
    assert_eq!(next(&mut answer).await.task_id, task);
    fake.set_extensions(&[]);
    let err = c
        .send_stream(steer(&ep, &task, "two", "m-2"))
        .await
        .err()
        .expect("the agent no longer lists it");
    assert!(matches!(err, AgentError::Unsupported(_)), "{err:?}");
    fake.release_gate();
}

#[tokio::test]
async fn the_agent_refuses_a_finished_task_an_unknown_one_and_another_context() {
    let fake = agent(&[STEER_EXTENSION]).await;
    let ep = fake.endpoint("steerable", None);
    let c = client();
    let (stream, task) = running(&c, &ep).await;

    // another context: the task is not found
    let mut other = steer(&ep, &task, "x", "m-other");
    other.context_id = "ctx-other".to_owned();
    let err = c.send_stream(other).await.err().expect("refused");
    assert!(matches!(err, AgentError::TaskNotFound(_)), "{err:?}");
    // an unknown task
    let err = c
        .send_stream(steer(&ep, "no-such-task", "x", "m-unknown"))
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, AgentError::TaskNotFound(_)), "{err:?}");

    // the task ends: UnsupportedOperation, which the orchestrator reads as "not delivered"
    fake.release_gate();
    let mut stream = stream;
    while let Some(env) = stream.next().await {
        if env.unwrap().task_state == Some(AgentTaskState::Completed) {
            break;
        }
    }
    eventually("the task is over for the agent", || async {
        c.send_stream(steer(&ep, &task, "late", "m-late"))
            .await
            .err()
            .filter(|e| matches!(e, AgentError::Unsupported(_)))
    })
    .await;
}

#[tokio::test]
async fn a_task_that_cannot_take_another_step_refuses() {
    let fake = agent(&[STEER_EXTENSION]).await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    // `gate` runs, but has no inbox for a steer
    let mut stream = c.send_stream(request(&ep, "gate work")).await.unwrap();
    let task = next(&mut stream).await.task_id;
    let err = c
        .send_stream(steer(&ep, &task, "hello", "m-steer"))
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, AgentError::Unsupported(_)), "{err:?}");
    fake.release_gate();
}
