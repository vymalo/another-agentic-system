//! The access tier end to end (ADR 0061, `access-tier/v1`), on both stores: the tier on an AG-UI
//! run is checked, recorded as sent, rides the outbox to the dispatcher, and reaches the agent in
//! the message metadata through the real A2A adapter, with the extension activated, only when the
//! agent's live card honours it; the tier the agent echoes is logged once on its status.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_core::{ACCESS_TIER_EXTENSION, AccessTier, STEER_EXTENSION};
use orch_testsupport::{CallKind, Chat, FakeAgentOptions, eventually};
use serde_json::{Value, json};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(20);

fn all() -> Value {
    json!({"tiers": ["plan", "edits", "auto", "bypass"], "default": "auto"})
}

/// A world whose `plain` agent lists `access-tier/v1` (and `steer/v1`) and whose `coder` does not.
async fn world(backend: Backend) -> World {
    World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                access_tier: Some(all()),
                extensions: vec![STEER_EXTENSION.to_owned()],
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

/// One run on `agent` with `text`, naming `tier` when given. The status; the stream is read to its
/// end when the run was accepted.
async fn run(
    chat: &Chat,
    agent: &str,
    thread: &str,
    run_id: &str,
    text: &str,
    tier: Option<&str>,
) -> u16 {
    let props = match tier {
        Some(tier) => json!({"forwardedProps": {"vymalo.tier": tier}}),
        None => json!({}),
    };
    let body = Chat::agui_input(thread, run_id, &[(&format!("m-{run_id}"), text)], props);
    // the raw answer: a refusal is a status to look at, not a panic
    let response = chat.agui_post(agent, &body).await;
    let status = response.status().as_u16();
    if status == 200 {
        tokio::time::timeout(WAIT, response.bytes())
            .await
            .expect("the run's stream ended")
            .unwrap();
    }
    status
}

fn tiers_logged(events: &[Value]) -> Vec<Option<String>> {
    events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .map(|e| e["data"]["tier"].as_str().map(str::to_owned))
        .collect()
}

fn echoed(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["kind"] == "agent_status")
        .filter_map(|e| e["data"]["tier"].as_str().map(str::to_owned))
        .collect()
}

async fn a_tier_reaches_the_agent_with_the_extension_activated_and_its_echo_is_logged(
    backend: Backend,
) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    assert_eq!(
        run(&chat, "plain", &thread, "run-1", "tiered go", Some("plan")).await,
        200
    );
    chat.wait_state(&thread, "done").await;

    // the agent: the tier under the URI in the message metadata, the URI activated twice
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].access_tier, Some(json!({"tier": "plan"})));
    assert!(calls[0].activates_access_tier(), "{:?}", calls[0]);
    // the log: the tier as sent, the agent's echo once, and its words
    let events = chat.events(&thread).await;
    assert_eq!(tiers_logged(&events), [Some("plan".to_owned())]);
    assert_eq!(echoed(&events), ["plan"]);
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "artifact" && e["data"]["text"] == "tiered: plan"),
        "{events:?}"
    );
    // a viewer: the user message says the tier in its START metadata
    let mut viewer = chat.agui_connect(&thread, None, false).await;
    let frames = viewer
        .frames_until(WAIT, |f| f.event["type"] == "RUN_FINISHED")
        .await;
    let start = frames
        .iter()
        .find(|f| f.event["type"] == "TEXT_MESSAGE_START" && f.event["role"] == "user")
        .expect("the user message");
    assert_eq!(start.event["metadata"]["vymalo.tier"], "plan");
}

async fn a_message_that_names_no_tier_is_the_message_it_always_was(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    for none in [None, Some("null")] {
        let id = format!("run-{}", none.unwrap_or("absent"));
        let sent = if none.is_some() {
            // `null` is a way to say none
            let body = Chat::agui_input(
                &thread,
                &id,
                &[(&format!("m-{id}"), "tiered again")],
                json!({"forwardedProps": {"vymalo.tier": null}}),
            );
            let mut sse = chat.agui_run("plain", &body).await;
            sse.collect_frames(WAIT).await;
            sse.status.as_u16()
        } else {
            run(&chat, "plain", &thread, &id, "tiered first", None).await
        };
        assert_eq!(sent, 200);
        chat.wait_state(&thread, "done").await;
    }
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    for call in &calls {
        assert_eq!(call.access_tier, None);
        assert!(!call.activates(ACCESS_TIER_EXTENSION), "{call:?}");
    }
    let events = chat.events(&thread).await;
    assert_eq!(tiers_logged(&events), [None, None]);
    assert!(echoed(&events).is_empty());
}

async fn each_message_names_its_own_tier(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    for (n, tier) in [(1, Some("bypass")), (2, None), (3, Some("edits"))] {
        let status = run(
            &chat,
            "plain",
            &thread,
            &format!("run-{n}"),
            &format!("tiered {n}"),
            tier,
        )
        .await;
        assert_eq!(status, 200);
        chat.wait_state(&thread, "done").await;
    }
    let sent: Vec<_> = world
        .plain
        .executions()
        .iter()
        .map(|c| c.access_tier.clone())
        .collect();
    assert_eq!(
        sent,
        [
            Some(json!({"tier": "bypass"})),
            None,
            Some(json!({"tier": "edits"}))
        ]
    );
    let events = chat.events(&thread).await;
    assert_eq!(
        tiers_logged(&events),
        [Some("bypass".to_owned()), None, Some("edits".to_owned())]
    );
    assert_eq!(echoed(&events), ["bypass", "edits"]);
}

async fn a_tier_the_card_does_not_honour_is_refused_before_anything_is_written(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    // an agent that lists nothing
    let thread = Uuid::now_v7().to_string();
    assert_eq!(
        run(&chat, "coder", &thread, "run-1", "echo hi", Some("plan")).await,
        422
    );
    assert_eq!(chat.get(&format!("/api/threads/{thread}")).await.0, 404);
    // an agent that lists some tiers but not this one
    world.plain.set_access_tier(Some(
        json!({"tiers": ["plan", "edits"], "default": "edits"}),
    ));
    let thread = Uuid::now_v7().to_string();
    assert_eq!(
        run(
            &chat,
            "plain",
            &thread,
            "run-1",
            "tiered hi",
            Some("bypass")
        )
        .await,
        422
    );
    assert_eq!(chat.get(&format!("/api/threads/{thread}")).await.0, 404);
    // a word that is not a tier
    let thread = Uuid::now_v7().to_string();
    assert_eq!(
        run(&chat, "plain", &thread, "run-1", "tiered hi", Some("yolo")).await,
        400
    );
    assert_eq!(chat.get(&format!("/api/threads/{thread}")).await.0, 404);
    // nothing reached either agent
    assert!(world.plain.calls().is_empty(), "{:?}", world.plain.calls());
    assert!(world.coder.calls().is_empty(), "{:?}", world.coder.calls());
}

async fn a_steered_message_carries_its_own_tier_into_the_running_task(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    // a task that waits for the steer
    let body = Chat::agui_input(
        &thread,
        "run-1",
        &[("m-1", "steerable refactor the parser")],
        json!({"forwardedProps": {"vymalo.tier": "plan"}}),
    );
    let sse = chat.agui_run("plain", &body).await;
    assert_eq!(sse.status.as_u16(), 200);
    chat.wait_state(&thread, "working").await;
    let body = Chat::agui_input(
        &thread,
        "run-2",
        &[("m-2", "faster please")],
        json!({"forwardedProps": {"vymalo.send": "steer", "vymalo.tier": "bypass"}}),
    );
    let steer = chat.agui_run("plain", &body).await;
    assert_eq!(steer.status.as_u16(), 200);
    eventually("the task reads the message", || async {
        chat.events(&thread)
            .await
            .iter()
            .any(|e| e["kind"] == "agent_message" && e["data"]["text"] == "steered: faster please")
            .then_some(())
    })
    .await;
    let calls = world.plain.calls();
    let steers: Vec<_> = calls.iter().filter(|c| c.kind == CallKind::Steer).collect();
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0].access_tier, Some(json!({"tier": "bypass"})));
    assert!(steers[0].activates(ACCESS_TIER_EXTENSION));
    assert!(steers[0].activates(STEER_EXTENSION));
    let events = chat.events(&thread).await;
    assert_eq!(
        tiers_logged(&events),
        [Some("plan".to_owned()), Some("bypass".to_owned())]
    );
    assert_eq!(AccessTier::ALL.len(), 4);
}

async fn the_rework_of_a_gate_reaches_the_agent_under_the_tier_of_the_job(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    let body = Chat::agui_input(
        &thread,
        "run-1",
        &[("m-1", "verify-red-once fix the login")],
        json!({"forwardedProps": {
            "vymalo.gate": {"require": ["agent-checks"]},
            "vymalo.tier": "edits"
        }}),
    );
    let mut sse = chat.agui_run("plain", &body).await;
    sse.collect_frames(WAIT).await;
    chat.wait_state(&thread, "done").await;

    // two deliveries: the person's message and the gate's rework, a new task of the same context
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[1].text.contains("attempt 2"), "{}", calls[1].text);
    for call in &calls {
        assert_eq!(call.access_tier, Some(json!({"tier": "edits"})), "{call:?}");
        assert!(call.activates_access_tier(), "{call:?}");
    }
    // the log says what each was sent under
    let events = chat.events(&thread).await;
    assert_eq!(tiers_logged(&events), [Some("edits".to_owned())]);
}

backends!(
    a_tier_reaches_the_agent_with_the_extension_activated_and_its_echo_is_logged,
    a_message_that_names_no_tier_is_the_message_it_always_was,
    each_message_names_its_own_tier,
    a_tier_the_card_does_not_honour_is_refused_before_anything_is_written,
    a_steered_message_carries_its_own_tier_into_the_running_task,
    the_rework_of_a_gate_reaches_the_agent_under_the_tier_of_the_job,
);
