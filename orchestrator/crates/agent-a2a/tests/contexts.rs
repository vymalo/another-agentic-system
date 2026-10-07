//! The context of a conversation is the agent's to assign (ADR 0055): the adapter's first message of a
//! conversation names none, and the agents below say what that costs or saves. One agent **refuses a
//! context it did not create** (what kagent does), one echoes the context it is given (what adam-rs's
//! runtime and most agents do); both start a conversation, and name it, when none is named.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, SendContent, SendRequest,
};
use orch_testsupport::{FakeAgent, FakeAgentOptions};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

fn request(ep: &AgentEndpoint, message: &str, context: Option<&str>) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: message.to_owned(),
        context_id: context.map(str::to_owned),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("echo hello".to_owned()),
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
            Err(_) => panic!("stream did not end; got {out:?}"),
        }
    }
}

async fn strict() -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        strict_contexts: true,
        ..FakeAgentOptions::default()
    })
    .await
}

/// What kagent 1.x does: a context the caller made up (the orchestrator used to send its thread id)
/// is refused, and nothing is started.
#[tokio::test]
async fn an_agent_that_refuses_a_context_it_did_not_create_refuses_the_threads_id() {
    let fake = strict().await;
    let ep = fake.endpoint("kagent", None);
    let c = client();
    let refused = match c
        .send_stream(request(&ep, "m-made-up", Some("0192-thread-id")))
        .await
    {
        Ok(stream) => {
            let mut stream = stream;
            stream.next().await.expect("an item").map(|_| ())
        }
        Err(e) => Err(e),
    };
    let err = refused.expect_err("a context the agent did not create");
    assert!(
        matches!(err, AgentError::Rejected(_)),
        "a permanent refusal, not a retry: {err:?}"
    );
    assert!(fake.calls().is_empty(), "nothing ran: {:?}", fake.calls());
}

/// The first message names no context, the agent assigns one, and every later message that names it
/// is taken: the whole conversation of a thread, with an agent that refuses what it did not create.
#[tokio::test]
async fn a_first_message_with_no_context_starts_a_conversation_the_agent_names() {
    let fake = strict().await;
    let ep = fake.endpoint("kagent", None);
    let c = client();

    let first = drain(c.send_stream(request(&ep, "m-1", None)).await.unwrap()).await;
    let context = first[0].context_id.clone();
    assert!(!context.is_empty(), "the agent assigned one: {first:?}");
    assert!(first.iter().all(|e| e.context_id == context));

    // the second message, in the context the agent assigned (what the binding holds by now)
    let second = drain(
        c.send_stream(request(&ep, "m-2", Some(&context)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(second[0].context_id, context, "the same conversation");
    assert_ne!(second[0].task_id, first[0].task_id, "a new task in it");

    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].requested_context, None,
        "the first message carried no contextId"
    );
    assert_eq!(
        calls[1].requested_context.as_deref(),
        Some(context.as_str()),
        "the second carried the one the agent assigned"
    );
}

/// An agent that echoes the context it is given (the default fake, adam-rs's runtime) still works
/// with a first message that names none, and with a thread that began earlier and was sent its id.
#[tokio::test]
async fn an_agent_that_echoes_contexts_takes_both_a_first_message_with_none_and_an_older_threads_id()
 {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("adam", None);
    let c = client();

    let fresh = drain(c.send_stream(request(&ep, "m-fresh", None)).await.unwrap()).await;
    assert!(!fresh[0].context_id.is_empty());

    // a thread that began before ADR 0055: its binding holds its own id, and it is sent as before
    let older = drain(
        c.send_stream(request(&ep, "m-older", Some("0192-thread-id")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(older[0].context_id, "0192-thread-id");
}

/// A message sent with no context is found by its id alone (the first message of a thread, after a
/// crash between sending and recording).
#[tokio::test]
async fn a_message_sent_with_no_context_is_found_by_its_id() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("adam", None);
    let c = client();
    let envs = drain(c.send_stream(request(&ep, "m-lost", None)).await.unwrap()).await;
    let task = envs[0].task_id.clone();
    assert_eq!(
        c.find_task_by_message(&ep, None, "m-lost").await.unwrap(),
        Some(task)
    );
    assert_eq!(
        c.find_task_by_message(&ep, None, "m-never").await.unwrap(),
        None
    );
}
