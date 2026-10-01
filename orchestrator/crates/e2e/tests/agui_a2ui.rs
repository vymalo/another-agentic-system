//! A2UI end to end (ADR 0013): a fake A2A agent that lists the A2UI extension in its card sends a
//! surface, the orchestrator relays it as an `a2ui-surface` activity to the run's requester and
//! to a viewer, the user's action comes back in `forwardedProps.a2uiAction`, and the agent gets
//! it as an `application/a2ui+json` data part of the same task. On the in-memory store and on
//! Postgres, through the real A2A adapter over HTTP. Every event that leaves a route is checked
//! against the vendored AG-UI schema.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_agui_proto::testkit::{assert_capabilities_json_conforms, assert_json_conforms};
use orch_core::{A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0, UI_CATALOG_EXTENSION};
use orch_testsupport::{
    Chat, FakeAgentOptions, Frame, SseClient, UI_CATALOG_ID, integral_numbers, ui_catalog,
    with_ui_catalog,
};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);

fn thread_id(n: u32) -> String {
    format!("00000000-0000-7000-8000-{:012}", 300 + n)
}

fn input(thread: &str, run: &str, messages: &[(&str, &str)], extra: Value) -> Value {
    Chat::agui_input(thread, run, messages, extra)
}

fn action_input(thread: &str, run: &str, user_action: Value) -> Value {
    input(
        thread,
        run,
        &[],
        json!({"forwardedProps": {"a2uiAction": {"userAction": user_action}}}),
    )
}

fn go() -> Value {
    json!({"name": "go", "surfaceId": "s1", "sourceComponentId": "go",
           "context": {"choice": "a"}, "timestamp": "2026-01-01T00:00:00Z"})
}

async fn whole(mut sse: SseClient) -> Vec<Frame> {
    assert_eq!(sse.status, 200);
    let frames = sse.collect_frames(WAIT).await;
    for frame in &frames {
        assert_json_conforms(&frame.event);
    }
    frames
}

fn ui_world(uris: &[&str]) -> Setup {
    Setup {
        plain: FakeAgentOptions {
            ui_extensions: uris.iter().map(|u| (*u).to_owned()).collect(),
            ..FakeAgentOptions::default()
        },
        ..Setup::default()
    }
}

fn surfaces(frames: &[Frame]) -> Vec<&Frame> {
    frames
        .iter()
        .filter(|f| f.event["activityType"] == "a2ui-surface")
        .collect()
}

async fn surface_then_action(backend: Backend) {
    let world = World::with(backend, ui_world(&[A2UI_EXTENSION_V0_9_1])).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(1);

    let first = whole(
        chat.agui_run(
            "plain",
            &input(&thread, "run-1", &[("m-1", "ui pick one")], json!({})),
        )
        .await,
    )
    .await;
    // Two payloads made the surface: the second snapshot is the whole surface, under the message
    // id of the first.
    let seen = surfaces(&first);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].event["messageId"], seen[1].event["messageId"]);
    assert_eq!(
        seen[0].event["content"]["a2ui_operations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let ops = seen[1].event["content"]["a2ui_operations"]
        .as_array()
        .unwrap();
    assert_eq!(ops.len(), 2);
    assert_eq!(seen[1].event["replace"], true);
    assert_eq!(
        first.last().unwrap().event["outcome"]["type"],
        "interrupt",
        "the surface accompanies the question"
    );

    // The agent was told what the renderer supports, and the extension was activated.
    let call = &world.plain.executions()[0];
    assert!(call.a2ui_capabilities.is_some(), "{call:?}");
    assert!(call.activates(A2UI_EXTENSION_V0_9_1));

    // A viewer reads the same surface, whole.
    let mut viewer = chat.agui_connect(&thread, None, true).await;
    assert_eq!(viewer.status, 200);
    let replay: Vec<Frame> = viewer.collect_frames(WAIT).await;
    assert_eq!(surfaces(&replay).len(), 2);
    assert_eq!(
        surfaces(&replay)[1].event["content"],
        seen[1].event["content"]
    );

    // The user acts.
    let second = whole(
        chat.agui_run("plain", &action_input(&thread, "run-2", go()))
            .await,
    )
    .await;
    assert_eq!(second[0].event["runId"], "run-2");
    assert!(
        second
            .iter()
            .any(|f| f.event["activityType"] == "vymalo.action"
                && f.event["content"]["name"] == "go")
    );
    assert_eq!(
        second.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );
    let answered = second
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.artifact")
        .unwrap();
    assert_eq!(answered.event["content"]["text"], "answered: ui-action go");

    // One task, continued, and the action reached the agent as A2UI.
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].task_id, calls[1].task_id);
    assert!(calls[1].resuming);
    assert_eq!(
        calls[1].actions,
        [json!({"version": "v0.9.1", "action": {
            "name": "go", "surfaceId": "s1", "sourceComponentId": "go",
            "timestamp": calls[1].actions[0]["action"]["timestamp"].clone(),
            "context": {"choice": "a"}}})]
    );
    let stamp = calls[1].actions[0]["action"]["timestamp"].as_str().unwrap();
    assert!(stamp.parse::<orch_core::Timestamp>().is_ok(), "{stamp}");

    // The chat log tells the same story.
    chat.wait_state(&thread, "done").await;
    let events = chat.events(&thread).await;
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert!(
        kinds.contains(&"ui_surface") && kinds.contains(&"ui_action"),
        "{kinds:?}"
    );
    assert_contiguous(&events);
}

async fn unknown_and_oversized_actions_never_reach_the_agent(backend: Backend) {
    let world = World::with(backend, ui_world(&[A2UI_EXTENSION_V0_9_1])).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(2);
    whole(
        chat.agui_run(
            "plain",
            &input(&thread, "run-1", &[("m-1", "ui pick one")], json!({})),
        )
        .await,
    )
    .await;
    let before = chat.events(&thread).await.len();

    let mut elsewhere = go();
    elsewhere["surfaceId"] = json!("elsewhere");
    let mut big = go();
    big["context"] = json!({"k": "v".repeat(20_000)});
    for (user_action, status) in [(elsewhere, 422), (big, 413)] {
        let resp = chat
            .agui_post("plain", &action_input(&thread, "run-x", user_action))
            .await;
        assert_eq!(resp.status().as_u16(), status);
        assert!(
            resp.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/problem+json")
        );
    }
    assert_eq!(chat.events(&thread).await.len(), before);
    assert_eq!(world.plain.executions().len(), 1);
}

async fn a_part_that_fails_the_envelope_is_an_error_line_and_the_run_goes_on(backend: Backend) {
    let world = World::with(backend, ui_world(&[A2UI_EXTENSION_V0_9_1])).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    for (n, script) in [(3, "ui-bad now"), (4, "ui-big now")] {
        let thread = thread_id(n);
        let frames = whole(
            chat.agui_run(
                "plain",
                &input(&thread, "run-1", &[("m-1", script)], json!({})),
            )
            .await,
        )
        .await;
        assert!(surfaces(&frames).is_empty(), "nothing unchecked is relayed");
        let error = frames
            .iter()
            .find(|f| f.event["activityType"] == "vymalo.error")
            .unwrap_or_else(|| panic!("{script}: no error line"));
        assert_eq!(error.event["content"]["retryable"], false);
        assert!(
            error.event["content"]["message"]
                .as_str()
                .unwrap()
                .contains("A2UI"),
            "{error:?}"
        );
        assert_eq!(
            frames.last().unwrap().event["outcome"],
            json!({"type": "success"}),
            "the run goes on to the end of the agent's turn"
        );
        let events = chat.events(&thread).await;
        assert!(events.iter().all(|e| e["kind"] != "ui_surface"));
        assert!(events.iter().any(|e| e["kind"] == "error"));
    }
}

async fn a_surface_in_a_message_and_in_the_question(backend: Backend) {
    let world = World::with(backend, ui_world(&[A2UI_EXTENSION_V0_9_1])).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let thread = thread_id(5);
    let frames = whole(
        chat.agui_run(
            "plain",
            &input(&thread, "run-1", &[("m-1", "ui-msg now")], json!({})),
        )
        .await,
    )
    .await;
    assert_eq!(surfaces(&frames).len(), 1);
    assert!(
        frames
            .iter()
            .any(|f| f.event["type"] == "TEXT_MESSAGE_CONTENT"
                && f.event["delta"] == "Here is a form")
    );

    let thread = thread_id(6);
    let frames = whole(
        chat.agui_run(
            "plain",
            &input(&thread, "run-1", &[("m-1", "ui-status now")], json!({})),
        )
        .await,
    )
    .await;
    assert_eq!(surfaces(&frames).len(), 1);
    let outcome = &frames.last().unwrap().event["outcome"];
    assert_eq!(outcome["type"], "interrupt");
    assert_eq!(outcome["interrupts"][0]["message"], "Which one?");
    // The surface is before the interrupt, inside the run.
    let surface_at = frames
        .iter()
        .position(|f| f.event["activityType"] == "a2ui-surface")
        .unwrap();
    assert!(surface_at < frames.len() - 3);
}

async fn a_deleted_surface_cannot_be_acted_on(backend: Backend) {
    let world = World::with(backend, ui_world(&[A2UI_EXTENSION_V0_9_1])).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(7);
    let frames = whole(
        chat.agui_run(
            "plain",
            &input(&thread, "run-1", &[("m-1", "ui-delete now")], json!({})),
        )
        .await,
    )
    .await;
    let s = surfaces(&frames);
    assert_eq!(s.len(), 2, "one snapshot per payload");
    let last = s[1].event["content"]["a2ui_operations"].as_array().unwrap();
    assert!(last.last().unwrap().get("deleteSurface").is_some());
    // The thread is done; and a surface that is gone takes no action even before that.
    chat.wait_state(&thread, "done").await;
    let mut a = go();
    a["surfaceId"] = json!("s2");
    let resp = chat
        .agui_post("plain", &action_input(&thread, "run-2", a))
        .await;
    assert!(
        matches!(resp.status().as_u16(), 409 | 422),
        "{}",
        resp.status()
    );
}

async fn capabilities_follow_the_live_card(backend: Backend) {
    let world = World::with(backend, ui_world(&[])).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let get = || async {
        let (status, doc) = chat.agui_capabilities("plain").await;
        assert_eq!(status, 200);
        assert_capabilities_json_conforms(&doc);
        doc
    };
    assert!(get().await.get("custom").is_none());
    world.plain.set_ui_extensions(&[A2UI_EXTENSION_V0_9_1]);
    let doc = get().await;
    assert!(doc["custom"][A2UI_EXTENSION_V0_9_1]["supportedCatalogIds"].is_array());
    world
        .plain
        .set_ui_extensions(&[A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0]);
    let doc = get().await;
    assert!(doc["custom"][A2UI_EXTENSION_V1_0]["supportedCatalogIds"].is_array());
    world.plain.set_ui_extensions(&[]);
    assert!(get().await.get("custom").is_none(), "never cached");
}

/// The `choices` story of the web (ADR 0023): the agent lists `ui-catalog/v1`, is told the screen's
/// catalog, draws a `Choices` under it and asks; the person's answers come back as one action named
/// `answer` whose `context.answers` the agent reads.
async fn choices_under_the_screens_catalog_and_the_answers_back(backend: Backend) {
    let world = World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                ui_extensions: vec![A2UI_EXTENSION_V0_9_1.to_owned()],
                extensions: vec![UI_CATALOG_EXTENSION.to_owned()],
                accepts_inline_catalogs: true,
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(7);

    let first = whole(
        chat.agui_run(
            "plain",
            &input(
                &thread,
                "run-1",
                &[("m-1", "choices now")],
                with_ui_catalog(2),
            ),
        )
        .await,
    )
    .await;
    let seen = surfaces(&first);
    assert_eq!(seen.len(), 2);
    let ops = seen[1].event["content"]["a2ui_operations"]
        .as_array()
        .unwrap();
    assert_eq!(ops[0]["createSurface"]["catalogId"], UI_CATALOG_ID);
    let components = ops[1]["updateComponents"]["components"].as_array().unwrap();
    let pick = components.iter().find(|c| c["id"] == "pick").unwrap();
    assert_eq!(pick["component"], "Choices");
    assert_eq!(pick["questions"].as_array().unwrap().len(), 3);
    assert_eq!(first.last().unwrap().event["outcome"]["type"], "interrupt");

    // the agent was told which catalog, and got it inline in this first message
    let catalog = ui_catalog(2);
    let calls = world.plain.executions();
    // (an A2A server reads the numbers of metadata as doubles: whole ones are made integers again)
    assert_eq!(
        calls[0].ui_catalog.as_ref().map(integral_numbers),
        Some(json!({
            "catalogId": UI_CATALOG_ID, "version": 2,
            "digest": catalog["digest"], "inline": true,
        }))
    );
    let inline: Vec<Value> = calls[0]
        .inline_catalogs
        .iter()
        .map(integral_numbers)
        .collect();
    assert_eq!(inline, [catalog["catalog"].clone()]);

    // the answers: one action, no message; the agent reads what was chosen
    let answers = json!([
        {"id": "db", "values": [], "other": "Cockroach"},
        {"id": "auth", "values": ["none"]},
        {"id": "deploy", "values": ["k8s", "compose"]},
    ]);
    let second = whole(
        chat.agui_run(
            "plain",
            &action_input(
                &thread,
                "run-2",
                json!({"name": "answer", "surfaceId": "s1", "sourceComponentId": "pick",
                       "context": {"answers": answers}, "timestamp": "2026-01-01T00:00:00Z"}),
            ),
        )
        .await,
    )
    .await;
    let answered = second
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.artifact")
        .unwrap();
    assert_eq!(
        answered.event["content"]["text"],
        "answered: ui-action answer db=other:Cockroach auth=none deploy=k8s,compose"
    );
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].task_id, calls[1].task_id, "the same task");
    assert_eq!(calls[1].actions[0]["action"]["context"]["answers"], answers);
    // the second message refers to the catalog and does not repeat it
    assert_eq!(calls[1].ui_catalog.as_ref().unwrap()["inline"], false);
    assert!(calls[1].inline_catalogs.is_empty());
}

backends!(
    choices_under_the_screens_catalog_and_the_answers_back,
    surface_then_action,
    unknown_and_oversized_actions_never_reach_the_agent,
    a_part_that_fails_the_envelope_is_an_error_line_and_the_run_goes_on,
    a_surface_in_a_message_and_in_the_question,
    a_deleted_surface_cannot_be_acted_on,
    capabilities_follow_the_live_card,
);
