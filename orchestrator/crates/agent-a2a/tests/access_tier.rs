//! The access tier of a message through the A2A adapter (`access-tier/v1`, ADR 0061, ADR 0008),
//! against an in-process A2A agent over real HTTP: the tier is in the message metadata under the
//! extension's URI and the URI is activated, only for an agent whose live card lists it with
//! well-formed params that honour the tier and only when the message names one; a card that no
//! longer honours the tier is sent **nothing**; any other agent is sent the message it always was.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    ACCESS_TIER_EXTENSION, AccessTier, AgentTaskState, AgentUpdate, STEER_EXTENSION, TierListing,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, SendContent, SendRequest,
};
use orch_testsupport::{Call, CallKind, FakeAgent, FakeAgentOptions};
use serde_json::{Value, json};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

fn all() -> Value {
    json!({"tiers": ["plan", "edits", "auto", "bypass"], "default": "auto"})
}

async fn agent(params: Option<Value>) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        access_tier: params,
        ..FakeAgentOptions::default()
    })
    .await
}

fn request(fake: &FakeAgent, text: &str, tier: Option<AccessTier>) -> SendRequest {
    let ep: AgentEndpoint = fake.endpoint("coder", None);
    SendRequest {
        endpoint: ep,
        message_id: "msg-1".to_owned(),
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
        tier,
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

async fn send(fake: &FakeAgent, req: SendRequest) -> (Call, Vec<AgentEnvelope>) {
    let seen = drain(client().send_stream(req).await.unwrap()).await;
    (fake.executions().pop().unwrap(), seen)
}

fn statuses(seen: &[AgentEnvelope]) -> Vec<(AgentTaskState, Option<AccessTier>)> {
    seen.iter()
        .filter_map(|e| match &e.update {
            Some(AgentUpdate::Status { state, tier, .. }) => Some((*state, *tier)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_card_is_read_with_its_tiers_and_default() {
    let fake = agent(Some(all())).await;
    let card = client()
        .read_card(&fake.endpoint("coder", None))
        .await
        .unwrap();
    let offer = card
        .access_tier
        .offer()
        .cloned()
        .expect("the card lists the extension");
    assert_eq!(offer.tiers(), AccessTier::ALL);
    assert_eq!(offer.default_tier(), AccessTier::Auto);
    // a card that lists nothing lists nothing; one that lists it badly is told apart from it, so
    // that a person capped below the agent's default is not run on an agent that cannot say its
    // tiers
    let fake = agent(None).await;
    let card = client()
        .read_card(&fake.endpoint("coder", None))
        .await
        .unwrap();
    assert_eq!(card.access_tier, TierListing::Absent);
    for params in [
        Some(json!({})),
        Some(json!({"tiers": ["plan"], "default": "bypass"})),
        Some(json!({"tiers": ["plan", "plan"], "default": "plan"})),
        Some(json!({"tiers": ["all"], "default": "all"})),
    ] {
        let fake = agent(params.clone()).await;
        let card = client()
            .read_card(&fake.endpoint("coder", None))
            .await
            .unwrap();
        assert_eq!(card.access_tier, TierListing::Malformed, "{params:?}");
    }
}

#[tokio::test]
async fn a_tier_the_card_honours_is_in_the_metadata_and_the_uri_is_activated() {
    for tier in AccessTier::ALL {
        let fake = agent(Some(all())).await;
        let (call, _) = send(&fake, request(&fake, "go", Some(tier))).await;
        assert_eq!(
            call.access_tier,
            Some(json!({"tier": tier.as_str()})),
            "{tier}"
        );
        assert!(
            call.activates_access_tier(),
            "header {:?} and message {:?}",
            call.extensions_header,
            call.message_extensions
        );
    }
}

#[tokio::test]
async fn a_message_that_names_no_tier_does_not_activate_the_extension() {
    let fake = agent(Some(all())).await;
    let (call, _) = send(&fake, request(&fake, "go", None)).await;
    assert_eq!(call.access_tier, None);
    assert!(!call.activates(ACCESS_TIER_EXTENSION));
    assert!(
        call.message_extensions
            .iter()
            .all(|e| e != ACCESS_TIER_EXTENSION)
    );
}

#[tokio::test]
async fn an_agent_that_does_not_list_the_extension_gets_the_request_it_always_got() {
    let plain = agent(None).await;
    let (call, _) = send(&plain, request(&plain, "go", None)).await;
    assert_eq!(call.access_tier, None);
    assert!(!call.activates(ACCESS_TIER_EXTENSION));
    assert!(
        call.extensions_header.is_empty(),
        "{:?}",
        call.extensions_header
    );
    assert!(call.message_extensions.is_empty());
}

#[tokio::test]
async fn a_tier_is_never_sent_to_a_card_that_does_not_honour_it() {
    let cases: [(Option<Value>, AccessTier); 4] = [
        // not listed
        (None, AccessTier::Plan),
        // listed, but not this tier
        (
            Some(json!({"tiers": ["plan"], "default": "plan"})),
            AccessTier::Edits,
        ),
        // listed badly: that is a card that does not list it
        (
            Some(json!({"tiers": ["plan"], "default": "auto"})),
            AccessTier::Plan,
        ),
        (Some(json!({"tiers": []})), AccessTier::Plan),
    ];
    for (params, tier) in cases {
        let fake = agent(params.clone()).await;
        let error = match client().send_stream(request(&fake, "go", Some(tier))).await {
            Err(error) => error,
            Ok(_) => panic!("{params:?}: the message was sent"),
        };
        assert!(matches!(error, AgentError::Unsupported(_)), "{error:?}");
        assert_eq!(
            error.public_detail(),
            format!("the agent no longer takes the access tier '{tier}'")
        );
        assert!(
            fake.calls().is_empty(),
            "nothing reached the agent: {:?}",
            fake.calls()
        );
    }
}

#[tokio::test]
async fn the_card_is_read_again_for_every_send_and_never_remembered() {
    let fake = agent(Some(all())).await;
    let (call, _) = send(&fake, request(&fake, "go", Some(AccessTier::Bypass))).await;
    assert!(call.activates_access_tier());
    // the agent stops listing bypass: the next send fails closed
    fake.set_access_tier(Some(
        json!({"tiers": ["plan", "edits"], "default": "edits"}),
    ));
    let mut again = request(&fake, "go", Some(AccessTier::Bypass));
    again.message_id = "msg-2".to_owned();
    assert!(matches!(
        client().send_stream(again).await,
        Err(AgentError::Unsupported(_))
    ));
    // and one it still lists goes
    let mut ok = request(&fake, "go", Some(AccessTier::Edits));
    ok.message_id = "msg-3".to_owned();
    let (call, _) = send(&fake, ok).await;
    assert_eq!(call.access_tier, Some(json!({"tier": "edits"})));
}

#[tokio::test]
async fn the_tier_the_agent_echoes_is_read_into_its_statuses() {
    let fake = agent(Some(all())).await;
    let (call, seen) = send(&fake, request(&fake, "tiered", Some(AccessTier::Plan))).await;
    assert_eq!(call.access_tier, Some(json!({"tier": "plan"})));
    let said = statuses(&seen);
    // the first status after the echo carries it, once
    assert_eq!(
        said.iter().filter(|(_, tier)| tier.is_some()).count(),
        1,
        "{said:?}"
    );
    assert_eq!(said[0].1, Some(AccessTier::Plan), "{said:?}");
    assert!(seen.iter().any(|e| matches!(
        &e.update,
        Some(AgentUpdate::Artifact { text: Some(t), .. }) if t == "tiered: plan"
    )));
    assert!(
        !seen
            .iter()
            .any(|e| e.update == Some(AgentUpdate::TierRejected))
    );
}

#[tokio::test]
async fn a_message_that_names_no_tier_gets_no_echo() {
    let fake = agent(Some(all())).await;
    let (_, seen) = send(&fake, request(&fake, "tiered", None)).await;
    assert!(statuses(&seen).iter().all(|(_, tier)| tier.is_none()));
    assert!(seen.iter().any(|e| matches!(
        &e.update,
        Some(AgentUpdate::Artifact { text: Some(t), .. }) if t == "tiered: none"
    )));
}

#[tokio::test]
async fn a_steer_carries_its_tier_beside_the_steer_extension() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        access_tier: Some(all()),
        extensions: vec![STEER_EXTENSION.to_owned()],
        ..FakeAgentOptions::default()
    })
    .await;
    let client = client();
    let mut first = request(&fake, "steerable work", Some(AccessTier::Auto));
    first.context_id = Some("ctx-1".to_owned());
    let mut stream = client.send_stream(first).await.unwrap();
    let task = loop {
        let env = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("an event in time")
            .expect("an event")
            .unwrap();
        if env.task_state == Some(AgentTaskState::Working) {
            break env.task_id;
        }
    };
    let mut steer = request(&fake, "faster", Some(AccessTier::Plan));
    steer.message_id = "m-steer".to_owned();
    steer.context_id = Some("ctx-1".to_owned());
    steer.task_id = Some(task);
    steer.steer = true;
    let mut answer = client.send_stream(steer).await.unwrap();
    let _ = answer.next().await.unwrap().unwrap();
    drop(answer);
    let calls = fake.calls();
    let steer = calls
        .iter()
        .find(|c| c.kind == CallKind::Steer)
        .expect("the steer reached the agent");
    assert_eq!(steer.access_tier, Some(json!({"tier": "plan"})));
    assert!(steer.activates(ACCESS_TIER_EXTENSION));
    assert!(steer.activates(STEER_EXTENSION));
    fake.release_gate();
}
