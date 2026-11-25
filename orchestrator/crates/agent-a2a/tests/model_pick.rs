//! The model and the reasoning effort of a message through the A2A adapter (`model-pick/v1` and
//! adam's `reasoning/v1`, ADR 0086, ADR 0008), against an in-process A2A agent over real HTTP: the
//! pick is in the message metadata under each extension's URI and the URI is activated, only for an
//! agent whose live card offers it and only when the message names it; a pick the card does not
//! offer is sent **nothing** (it fails closed, unlike a locale); a message that names none activates
//! nothing and carries no member.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    Classify, ErrorClass, MODEL_PICK_EXTENSION, ModelId, ModelPick, REASONING_EXTENSION,
    ReasoningEffort,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, SendContent, SendRequest,
};
use orch_testsupport::{Call, FakeAgent, FakeAgentOptions};
use serde_json::{Value, json};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

fn both() -> Vec<(String, Value)> {
    vec![
        (
            MODEL_PICK_EXTENSION.to_owned(),
            json!({"choices": [{"id": "fast", "label": "Fast"}, {"id": "deep", "label": "Deep"}], "default": "fast"}),
        ),
        (
            REASONING_EXTENSION.to_owned(),
            json!({"efforts": ["low", "high"], "default": "low"}),
        ),
    ]
}

async fn agent(card_extensions: Vec<(String, Value)>) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        card_extensions,
        ..FakeAgentOptions::default()
    })
    .await
}

fn request(fake: &FakeAgent, pick: Option<ModelPick>) -> SendRequest {
    let ep: AgentEndpoint = fake.endpoint("coder", None);
    SendRequest {
        endpoint: ep,
        message_id: "msg-1".to_owned(),
        context_id: None,
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("go".to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick,
    }
}

fn deep_high() -> ModelPick {
    ModelPick {
        model: Some(ModelId::parse("deep").unwrap()),
        reasoning_effort: Some(ReasoningEffort::High),
    }
}

async fn drain(mut stream: AgentStream) -> Vec<AgentEnvelope> {
    let mut seen = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(item)) => seen.push(item.unwrap()),
            Ok(None) => return seen,
            Err(_) => panic!("the stream did not end"),
        }
    }
}

async fn send(fake: &FakeAgent, req: SendRequest) -> Call {
    drain(client().send_stream(req).await.unwrap()).await;
    fake.executions().pop().unwrap()
}

fn member(call: &Call, uri: &str) -> Option<Value> {
    call.message_metadata.as_ref()?.get(uri).cloned()
}

fn activated(call: &Call, uri: &str) -> bool {
    call.activates(uri) && call.message_extensions.iter().any(|e| e == uri)
}

#[tokio::test]
async fn the_card_is_read_with_its_choices_and_levels() {
    let fake = agent(both()).await;
    let card = client()
        .read_card(&fake.endpoint("coder", None))
        .await
        .unwrap();
    assert_eq!(card.pick.model.offer().unwrap().ids(), ["fast", "deep"]);
    assert_eq!(
        card.pick.effort.offer().unwrap().efforts,
        [ReasoningEffort::Low, ReasoningEffort::High]
    );
}

#[tokio::test]
async fn a_pick_the_card_offers_arrives_in_the_metadata_and_each_uri_is_activated() {
    let fake = agent(both()).await;
    let call = send(&fake, request(&fake, Some(deep_high()))).await;
    assert_eq!(
        member(&call, MODEL_PICK_EXTENSION),
        Some(json!({"model": "deep"}))
    );
    assert_eq!(
        member(&call, REASONING_EXTENSION),
        Some(json!({"effort": "high"}))
    );
    assert!(
        activated(&call, MODEL_PICK_EXTENSION),
        "{:?}",
        call.extensions_header
    );
    assert!(
        activated(&call, REASONING_EXTENSION),
        "{:?}",
        call.extensions_header
    );
}

#[tokio::test]
async fn an_effort_alone_activates_only_the_reasoning_extension() {
    let fake = agent(both()).await;
    let call = send(
        &fake,
        request(&fake, Some(ModelPick::effort(ReasoningEffort::Low))),
    )
    .await;
    assert_eq!(
        member(&call, REASONING_EXTENSION),
        Some(json!({"effort": "low"}))
    );
    assert_eq!(member(&call, MODEL_PICK_EXTENSION), None);
    assert!(!call.activates(MODEL_PICK_EXTENSION));
}

#[tokio::test]
async fn a_message_that_names_no_pick_activates_nothing_and_carries_no_member() {
    let fake = agent(both()).await;
    let call = send(&fake, request(&fake, None)).await;
    for uri in [MODEL_PICK_EXTENSION, REASONING_EXTENSION] {
        assert_eq!(member(&call, uri), None, "{uri}");
        assert!(!call.activates(uri), "{uri}");
        assert!(call.message_extensions.iter().all(|e| e != uri), "{uri}");
    }
}

#[tokio::test]
async fn a_pick_the_card_does_not_offer_is_never_sent() {
    let fake = agent(both()).await;
    for pick in [
        ModelPick::model("huge").unwrap(),
        ModelPick::effort(ReasoningEffort::Max),
    ] {
        let error = client()
            .send_stream(request(&fake, Some(pick)))
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(error, AgentError::Unsupported(_)), "{error:?}");
        assert_eq!(error.class(), ErrorClass::Unsupported, "never retried");
    }
    assert!(fake.executions().is_empty(), "nothing reached the agent");
}

#[tokio::test]
async fn an_agent_that_lists_neither_refuses_a_pick_and_is_sent_nothing() {
    let fake = agent(Vec::new()).await;
    let error = client()
        .send_stream(request(&fake, Some(deep_high())))
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(error, AgentError::Unsupported(_)), "{error:?}");
    assert!(fake.executions().is_empty());
    // and the plain message it has always taken still goes
    let call = send(&fake, request(&fake, None)).await;
    assert_eq!(member(&call, MODEL_PICK_EXTENSION), None);
}
