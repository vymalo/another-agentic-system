//! A person's half of `child-sessions/v1` through the A2A adapter (ADR 0072, S3c): a start and a
//! close are messages whose `metadata` holds the request under the URI, activated by the header and
//! the message's own `extensions`, with no `taskId`; both are sent only to an agent whose live card
//! offers the extension and fail closed (nothing sent) otherwise; the agent's refusals keep their
//! words and their class.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{Classify as _, ErrorClass};
use orch_ports::{AgentClient, AgentEndpoint, AgentError, AgentStream, SendContent, SendRequest};
use orch_testsupport::{FakeAgent, FakeAgentOptions};
use serde_json::{Value, json};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

fn offer() -> Value {
    json!({"agents": [{"name": "planner", "description": "Plans."}], "maxOpen": 4})
}

async fn agent(params: Option<Value>) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        child_sessions: params,
        ..FakeAgentOptions::default()
    })
    .await
}

fn request(ep: &AgentEndpoint, context: &str, content: SendContent) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: "msg-person".to_owned(),
        context_id: Some(context.to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content,
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
    }
}

fn start(keep_open: bool) -> SendContent {
    SendContent::SessionStart {
        text: "List every writer of the header.".to_owned(),
        agent: "planner".to_owned(),
        description: "Find the writers".to_owned(),
        keep_open,
    }
}

async fn drain(mut stream: AgentStream) {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(item)) => {
                item.unwrap();
            }
            Ok(None) => return,
            Err(_) => panic!("the stream did not end"),
        }
    }
}

async fn send(client: &A2aAgentClient, req: SendRequest) -> Result<(), AgentError> {
    let stream = client.send_stream(req).await?;
    drain(stream).await;
    Ok(())
}

fn the_call(fake: &FakeAgent) -> orch_testsupport::Call {
    let calls = fake.executions();
    assert_eq!(calls.len(), 1, "{calls:?}");
    calls.into_iter().next().unwrap()
}

#[tokio::test]
async fn a_start_carries_its_request_under_the_uri_activated_and_with_no_task() {
    let fake = agent(Some(offer())).await;
    let ep = fake.endpoint("coder", None);
    let mut req = request(&ep, "ctx-1", start(true));
    // a start is never a turn of a task, whatever the request names
    req.task_id = Some("t-parent".to_owned());
    send(&client(), req).await.unwrap();

    let call = the_call(&fake);
    assert_eq!(
        call.child_sessions,
        Some(
            json!({"start": {"agent": "planner", "description": "Find the writers", "keepOpen": true}})
        )
    );
    assert_eq!(call.text, "List every writer of the header.");
    assert_eq!(call.requested_task, None);
    assert_eq!(call.requested_context.as_deref(), Some("ctx-1"));
    assert!(call.activates_child_sessions(), "header and message");
}

#[tokio::test]
async fn a_start_that_is_not_kept_open_says_so() {
    let fake = agent(Some(offer())).await;
    let ep = fake.endpoint("coder", None);
    send(&client(), request(&ep, "ctx-1", start(false)))
        .await
        .unwrap();
    assert_eq!(
        the_call(&fake).child_sessions,
        Some(
            json!({"start": {"agent": "planner", "description": "Find the writers", "keepOpen": false}})
        )
    );
}

#[tokio::test]
async fn a_close_is_one_text_part_and_a_close_member_to_the_child_context() {
    let fake = agent(Some(offer())).await;
    let ep = fake.endpoint("coder", None);
    for cancel in [false, true] {
        send(
            &client(),
            request(&ep, "ctx-k", SendContent::SessionClose { cancel }),
        )
        .await
        .unwrap();
    }
    let calls = fake.executions();
    assert_eq!(calls.len(), 2);
    for (call, cancel) in calls.iter().zip([false, true]) {
        assert_eq!(
            call.child_sessions,
            Some(json!({"close": {"cancel": cancel}}))
        );
        assert_eq!(call.text, "close");
        assert_eq!(call.requested_task, None);
        assert_eq!(call.requested_context.as_deref(), Some("ctx-k"));
        assert!(call.activates_child_sessions());
    }
}

#[tokio::test]
async fn nothing_is_sent_when_the_card_does_not_offer_the_extension() {
    for params in [None, Some(json!({"agents": [], "maxOpen": 4}))] {
        let fake = agent(params).await;
        let ep = fake.endpoint("coder", None);
        for content in [start(true), SendContent::SessionClose { cancel: false }] {
            let err = send(&client(), request(&ep, "ctx-1", content))
                .await
                .expect_err("fails closed");
            assert!(matches!(err, AgentError::Unsupported(_)), "{err:?}");
            assert_eq!(err.class(), ErrorClass::Unsupported);
        }
        assert_eq!(fake.rpc_count("send_streaming_message"), 0);
        assert!(fake.calls().is_empty());
    }
}

#[tokio::test]
async fn a_plain_message_carries_no_session_request() {
    let fake = agent(Some(offer())).await;
    let ep = fake.endpoint("coder", None);
    send(
        &client(),
        request(&ep, "ctx-1", SendContent::Text("hello".to_owned())),
    )
    .await
    .unwrap();
    assert_eq!(the_call(&fake).child_sessions, None);
}

#[tokio::test]
async fn the_refusals_of_the_agent_keep_their_words_and_their_class() {
    let fake = agent(Some(offer())).await;
    let ep = fake.endpoint("coder", None);

    // the agent's limit of working rounds: UnsupportedOperation with words for the person
    let mut req = request(&ep, "ctx-1", start(true));
    if let SendContent::SessionStart { text, .. } = &mut req.content {
        *text = "[mock:refuse-working] go".to_owned();
    }
    let err = send(&client(), req).await.expect_err("refused");
    assert!(
        matches!(&err, AgentError::Unsupported(w) if w.contains("4 rounds are already working")),
        "{err:?}"
    );

    // a start with no description: InvalidParams with words
    let mut req = request(&ep, "ctx-1", start(true));
    if let SendContent::SessionStart { description, .. } = &mut req.content {
        description.clear();
    }
    let err = send(&client(), req).await.expect_err("refused");
    assert!(
        matches!(&err, AgentError::Rejected(w) if w.contains("`start.description` is missing")),
        "{err:?}"
    );
    assert_eq!(err.class(), ErrorClass::Invalid);
    // neither started a run
    assert!(fake.executions().is_empty());
}
