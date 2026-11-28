//! The mode of a message through the A2A adapter (`agent-mode/v1`, ADR 0109, ADR 0008), against an
//! in-process A2A agent over real HTTP: the mode is in the message metadata under the extension's
//! URI and the URI is activated, only for an agent whose live card offers it and only when the
//! message names it; a mode the card does not offer is sent **nothing** (it fails closed); a
//! message that names none activates nothing and carries no member.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{Classify, ErrorClass, MODE_EXTENSION, ModeName};
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

fn card() -> Vec<(String, Value)> {
    vec![(
        MODE_EXTENSION.to_owned(),
        json!({
            "modes": [
                {"name": "build", "title": "Build", "description": "Make the change."},
                {"name": "plan", "title": "Plan", "description": "Read and plan."}
            ],
            "default": "build"
        }),
    )]
}

async fn agent(card_extensions: Vec<(String, Value)>) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        card_extensions,
        ..FakeAgentOptions::default()
    })
    .await
}

fn plan() -> ModeName {
    ModeName::parse("plan").unwrap()
}

fn request(fake: &FakeAgent, mode: Option<ModeName>) -> SendRequest {
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
        pick: None,
        mode,
        prompt_signature: None,
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
async fn the_card_is_read_with_its_modes_and_default() {
    let fake = agent(card()).await;
    let card = client()
        .read_card(&fake.endpoint("coder", None))
        .await
        .unwrap();
    let offer = card.mode.offer().expect("a mode offer");
    assert_eq!(offer.names(), ["build", "plan"]);
    assert_eq!(offer.default, ModeName::parse("build").unwrap());
}

#[tokio::test]
async fn a_mode_the_card_offers_arrives_in_the_metadata_and_the_uri_is_activated() {
    let fake = agent(card()).await;
    let call = send(&fake, request(&fake, Some(plan()))).await;
    assert_eq!(member(&call, MODE_EXTENSION), Some(json!({"mode": "plan"})));
    assert!(
        activated(&call, MODE_EXTENSION),
        "{:?}",
        call.extensions_header
    );
}

#[tokio::test]
async fn a_message_that_names_none_activates_nothing_and_carries_no_member() {
    let fake = agent(card()).await;
    let call = send(&fake, request(&fake, None)).await;
    assert_eq!(member(&call, MODE_EXTENSION), None);
    assert!(!activated(&call, MODE_EXTENSION));
    assert!(
        call.extensions_header
            .iter()
            .all(|uri| uri != MODE_EXTENSION),
        "{:?}",
        call.extensions_header
    );
}

#[tokio::test]
async fn a_mode_the_card_does_not_offer_is_refused_and_sends_nothing() {
    let fake = agent(card()).await;
    let error = client()
        .send_stream(request(&fake, Some(ModeName::parse("review").unwrap())))
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(error, AgentError::Unsupported(_)), "{error:?}");
    assert_eq!(error.class(), ErrorClass::Unsupported, "never retried");
    assert!(fake.executions().is_empty(), "nothing was sent");
}

#[tokio::test]
async fn a_card_that_lists_the_extension_badly_is_read_as_not_listed() {
    let fake = agent(vec![(
        MODE_EXTENSION.to_owned(),
        json!({"modes": [], "default": "build"}),
    )])
    .await;
    let card = client()
        .read_card(&fake.endpoint("coder", None))
        .await
        .unwrap();
    assert!(card.mode.offer().is_none());
    let error = client()
        .send_stream(request(&fake, Some(plan())))
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(error, AgentError::Unsupported(_)), "{error:?}");
    assert!(fake.executions().is_empty(), "nothing was sent");
}
