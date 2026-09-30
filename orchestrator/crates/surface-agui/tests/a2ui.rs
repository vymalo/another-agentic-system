//! A2UI over HTTP (ADR 0013): the surface an agent sends reaches viewers whole, an action comes
//! back in `forwardedProps.a2uiAction` and goes to the agent on the same task, and everything
//! else about actions is refused before the stream with the right status.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::RELEASE_CHANNELS_URI;
use orch_agui_proto::testkit::assert_capabilities_json_conforms;
use orch_core::{A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0, MAX_ACTION_CONTEXT_BYTES, UiVersion};
use orch_ports::memory::Call;
use serde_json::{Value, json};
use support::*;

fn go() -> Value {
    json!({"name": "go", "surfaceId": "s1", "sourceComponentId": "go",
           "context": {"choice": "a"}, "timestamp": "2026-09-29T10:00:00Z"})
}

fn action_run(thread: &str, run: &str, user_action: Value) -> Value {
    input_with(
        thread,
        run,
        &[],
        json!({"forwardedProps": {"a2uiAction": {"userAction": user_action}}}),
    )
}

/// A thread that has asked "Pick one" with a surface `s1`; returns its id.
async fn blocked_with_surface(h: &Harness) -> String {
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m-1", "ui pick one")]),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().event["outcome"]["type"], "interrupt");
    thread
}

fn surface_snapshots(frames: &[Frame]) -> Vec<&Frame> {
    frames
        .iter()
        .filter(|f| f.event["activityType"] == "a2ui-surface")
        .collect()
}

#[tokio::test]
async fn a_surface_reaches_the_requester_whole_and_the_question_follows() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m-1", "ui pick one")]),
        )
        .await
        .all()
        .await;
    let surfaces = surface_snapshots(&frames);
    assert_eq!(surfaces.len(), 1);
    let s = &surfaces[0].event;
    assert_eq!(s["messageId"], "a2ui-3");
    assert_eq!(s["replace"], true);
    let ops = s["content"]["a2ui_operations"].as_array().unwrap();
    assert_eq!(ops.len(), 2);
    assert!(ops[0]["createSurface"]["surfaceId"] == "s1");
    assert_eq!(s["metadata"]["vymalo.actor"]["type"], "agent");
    assert_eq!(frames.last().unwrap().event["outcome"]["type"], "interrupt");

    // A viewer who connects later reads the same surface, whole.
    let replay = h.connect_run(&thread, ALICE, None).await.all().await;
    let viewer = surface_snapshots(&replay);
    assert_eq!(viewer.len(), 1);
    assert_eq!(viewer[0].event["content"], s["content"]);
}

#[tokio::test]
async fn an_action_is_delivered_to_the_same_task_and_answers_the_wait() {
    let h = Harness::start().await;
    let thread = blocked_with_surface(&h).await;

    let frames = h
        .run("plain", ALICE, &action_run(&thread, "run-act", go()))
        .await
        .all()
        .await;
    assert_eq!(frames[0].kind(), "RUN_STARTED");
    assert_eq!(frames[0].event["runId"], "run-act");
    let action = frames
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.action")
        .expect("the requester is shown what it did");
    let mut content = action.event["content"].clone();
    assert!(
        content
            .as_object_mut()
            .unwrap()
            .remove("at")
            .is_some_and(|at| at.is_string()),
        "every vymalo activity says when"
    );
    assert_eq!(
        content,
        json!({"surfaceId": "s1", "name": "go", "sourceComponentId": "go",
               "context": {"choice": "a"}})
    );
    assert_eq!(action.event["metadata"]["vymalo.actor"]["type"], "user");
    assert!(
        !kinds(&frames).contains(&"TEXT_MESSAGE_START"),
        "an action is not a message"
    );
    assert_eq!(
        frames.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );

    // The log: a ui_action by the user, the agent's answer, done.
    let events = h.events(ALICE, &thread).await;
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "user_message",
            "agent_status",
            "ui_surface",
            "agent_status",
            "thread_state",
            "ui_action",
            "agent_status",
            "artifact",
            "agent_status",
            "thread_state"
        ]
    );
    let logged = &events[5];
    assert_eq!(logged["actor"]["type"], "user");
    assert_eq!(logged["data"]["runId"], "run-act");
    assert_eq!(logged["data"]["version"], "v0.9.1");
    assert!(
        logged["data"].get("timestamp").is_none(),
        "the log's own time is the action's time"
    );

    // The agent got an action, on the task it was waiting on.
    let sends = h.agent.sends();
    assert_eq!(sends.len(), 2);
    let Call::Send {
        task_id, action, ..
    } = &sends[1]
    else {
        panic!("{sends:?}");
    };
    assert!(task_id.is_some());
    let action = action.as_ref().unwrap();
    assert_eq!(action.name, "go");
    assert_eq!(action.surface_id, "s1");
    assert_eq!(action.source_component_id, "go");
    assert_eq!(action.version, UiVersion::V0_9_1);
}

#[tokio::test]
async fn an_action_for_a_surface_the_thread_does_not_have_is_a_422_and_nothing_happens() {
    let h = Harness::start().await;
    let thread = blocked_with_surface(&h).await;
    let before = h.events(ALICE, &thread).await.len();
    let mut other = go();
    other["surfaceId"] = json!("somewhere-else");
    let r = h
        .refused("plain", Some(ALICE), &action_run(&thread, "run-x", other))
        .await;
    let p = r.problem(422);
    assert!(p["detail"].as_str().unwrap().contains("somewhere-else"));
    assert_eq!(h.events(ALICE, &thread).await.len(), before);
    assert_eq!(h.agent.sends().len(), 1, "nothing was sent to the agent");
    assert_eq!(h.state(ALICE, &thread).await, "blocked");
}

#[tokio::test]
async fn an_action_on_a_new_thread_has_no_surface_to_act_on() {
    let h = Harness::start().await;
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &action_run(&new_thread_id(), "run-x", go()),
        )
        .await;
    r.problem(422);
    assert!(h.agent.sends().is_empty());
}

#[tokio::test]
async fn malformed_and_oversized_actions_are_refused_with_the_right_status() {
    let h = Harness::start().await;
    let thread = blocked_with_surface(&h).await;
    let before = h.events(ALICE, &thread).await.len();

    for (user_action, status) in [
        (json!("go"), 422),
        (json!({"name": "go", "surfaceId": "s1"}), 422),
        (
            json!({"name": 1, "surfaceId": "s1", "sourceComponentId": "go"}),
            422,
        ),
        (
            json!({"name": "go", "surfaceId": "s1", "sourceComponentId": "go", "context": [1]}),
            422,
        ),
        (
            json!({"name": "n".repeat(300), "surfaceId": "s1", "sourceComponentId": "go"}),
            413,
        ),
        (
            json!({"name": "go", "surfaceId": "s1", "sourceComponentId": "go",
                   "context": {"k": "v".repeat(MAX_ACTION_CONTEXT_BYTES)}}),
            413,
        ),
    ] {
        let r = h
            .refused(
                "plain",
                Some(ALICE),
                &action_run(&thread, "run-x", user_action.clone()),
            )
            .await;
        r.problem(status);
    }
    // No `userAction` at all.
    let body = input_with(
        &thread,
        "run-x",
        &[],
        json!({"forwardedProps": {"a2uiAction": {"name": "go"}}}),
    );
    h.refused("plain", Some(ALICE), &body).await.problem(422);

    assert_eq!(h.events(ALICE, &thread).await.len(), before);
    assert_eq!(h.agent.sends().len(), 1);
}

#[tokio::test]
async fn an_action_beside_a_message_is_ambiguous() {
    let h = Harness::start().await;
    let thread = blocked_with_surface(&h).await;
    let body = input_with(
        &thread,
        "run-x",
        &[("m-1", "ui pick one"), ("m-2", "and also this")],
        json!({"forwardedProps": {"a2uiAction": {"userAction": go()}}}),
    );
    h.refused("plain", Some(ALICE), &body).await.problem(422);
    assert_eq!(h.agent.sends().len(), 1);
}

#[tokio::test]
async fn an_action_follows_the_rules_of_ownership_and_state() {
    let h = Harness::start().await;
    let thread = blocked_with_surface(&h).await;

    // Someone else's thread is a 404, like any input.
    h.refused("plain", Some(BOB), &action_run(&thread, "run-x", go()))
        .await
        .problem(404);
    // The URL's agent must be the thread's.
    h.refused("coder", Some(ALICE), &action_run(&thread, "run-x", go()))
        .await
        .problem(409);
    // No identity, no action.
    h.refused("plain", None, &action_run(&thread, "run-x", go()))
        .await
        .problem(401);
    assert_eq!(h.agent.sends().len(), 1);

    // Answered once, the thread finishes; then it takes no action.
    h.run("plain", ALICE, &action_run(&thread, "run-act", go()))
        .await
        .all()
        .await;
    h.wait_state(ALICE, &thread, "done").await;
    h.refused("plain", Some(ALICE), &action_run(&thread, "run-y", go()))
        .await
        .problem(409);
}

#[tokio::test]
async fn an_action_run_id_is_never_reused() {
    let h = Harness::start().await;
    let thread = blocked_with_surface(&h).await;
    h.refused("plain", Some(ALICE), &action_run(&thread, "run-1", go()))
        .await
        .problem(422);
    assert_eq!(h.agent.sends().len(), 1);
}

#[tokio::test]
async fn the_capabilities_declare_a2ui_only_while_the_live_card_lists_it() {
    let h = Harness::start().await;
    let get = || async {
        let doc = h.get("/agui/agents/plain/capabilities", Some(ALICE)).await;
        assert_eq!(doc.status, 200);
        let doc = doc.json();
        assert_capabilities_json_conforms(&doc);
        doc
    };
    assert!(
        get().await.get("custom").is_none(),
        "the card does not list A2UI"
    );

    h.agent.set_ui("plain", &[UiVersion::V0_9_1]);
    let doc = get().await;
    let ids = &doc["custom"][A2UI_EXTENSION_V0_9_1]["supportedCatalogIds"];
    assert!(ids.as_array().is_some_and(|a| !a.is_empty()), "{doc}");
    assert!(doc["custom"].get(A2UI_EXTENSION_V1_0).is_none());
    assert!(doc["custom"].get(RELEASE_CHANNELS_URI).is_none());

    h.agent
        .set_ui("plain", &[UiVersion::V1_0, UiVersion::V0_9_1]);
    let doc = get().await;
    assert!(doc["custom"].get(A2UI_EXTENSION_V0_9_1).is_some());
    assert!(doc["custom"].get(A2UI_EXTENSION_V1_0).is_some());

    // The card cannot be read: fail closed, nothing is declared.
    h.agent.set_card_down("plain", true);
    assert!(get().await.get("custom").is_none());
    h.agent.set_card_down("plain", false);

    h.agent.set_ui("plain", &[]);
    assert!(get().await.get("custom").is_none(), "never cached");
}
