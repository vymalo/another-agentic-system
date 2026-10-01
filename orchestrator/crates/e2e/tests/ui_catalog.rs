//! The UI catalog end to end (ADR 0023), on both stores: the web's catalog on an AG-UI run reaches
//! the log, first in its commit and once per digest, and the state snapshot says which one is
//! current, through the real route, dispatcher and A2A adapter against the fake agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_core::{A2UI_EXTENSION_V0_9_1, THREAD_TOOLS_EXTENSION, UI_CATALOG_EXTENSION};
use orch_testsupport::{
    Chat, FakeAgentOptions, Frame, UI_CATALOG_ID, integral_numbers, ui_catalog, with_ui_catalog,
};
use serde_json::{Value, json};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(20);

/// One run of `message` under `run`, to its end: the frames.
async fn run(chat: &Chat, thread: &str, run: &str, message: &str, extra: Value) -> Vec<Frame> {
    let body = Chat::agui_input(thread, run, &[(&format!("m-{run}"), message)], extra);
    let mut sse = chat.agui_run("plain", &body).await;
    assert_eq!(sse.status, 200);
    sse.collect_frames(WAIT).await
}

fn kinds(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["kind"].as_str().unwrap()).collect()
}

/// The `thread.uiCatalog` of each `STATE_SNAPSHOT` in `frames`.
fn snapshot_catalogs(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter(|f| f.event["type"] == "STATE_SNAPSHOT")
        .map(|f| f.event["snapshot"]["thread"]["uiCatalog"].clone())
        .collect()
}

fn reference(version: u32) -> Value {
    let catalog = ui_catalog(version);
    json!({"catalogId": catalog["catalogId"], "version": version, "digest": catalog["digest"]})
}

async fn the_screens_catalog_is_recorded_once_per_digest_and_the_snapshot_names_the_newest(
    backend: Backend,
) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();

    // the first run of the thread carries version 1: one event, first in its commit
    let frames = run(&chat, &thread, "run-1", "echo one", with_ui_catalog(1)).await;
    assert_eq!(frames[0].event["type"], "RUN_STARTED");
    let snapshots = snapshot_catalogs(&frames);
    assert!(!snapshots.is_empty());
    assert!(
        snapshots.iter().all(|c| *c == reference(1)),
        "{snapshots:?}"
    );
    chat.wait_state(&thread, "done").await;
    let events = chat.events(&thread).await;
    assert_eq!(kinds(&events)[..2], ["ui_catalog", "user_message"]);
    assert_eq!(events[0]["data"], ui_catalog(1));
    assert_eq!(events[0]["actor"]["type"], "user");

    // the same request again: an attach, nothing written twice
    let before = chat.events(&thread).await;
    let again = run(&chat, &thread, "run-1", "echo one", with_ui_catalog(1)).await;
    assert_eq!(again[0].event["type"], "RUN_STARTED");
    assert_eq!(chat.events(&thread).await, before);
    assert_eq!(world.plain.executions().len(), 1, "nothing was sent twice");

    // version 2 on the next job: a second event, and the snapshots name version 2
    let frames = run(&chat, &thread, "run-2", "echo two", with_ui_catalog(2)).await;
    assert!(
        snapshot_catalogs(&frames)
            .iter()
            .all(|c| *c == reference(2))
    );
    wait_for_jobs(&chat, &thread, 2).await;
    let events = chat.events(&thread).await;
    assert_eq!(count(&events, "ui_catalog"), 2);
    let second = events
        .iter()
        .rposition(|e| e["kind"] == "ui_catalog")
        .unwrap();
    assert_eq!(events[second + 1]["kind"], "user_message");
    assert_eq!(events[second + 2]["kind"], "job_started");

    // the older screen comes back with version 1: known, so nothing is written, and the thread
    // keeps the newest
    let frames = run(&chat, &thread, "run-3", "echo three", with_ui_catalog(1)).await;
    assert!(
        snapshot_catalogs(&frames)
            .iter()
            .all(|c| *c == reference(2))
    );
    wait_for_jobs(&chat, &thread, 3).await;
    assert_eq!(count(&chat.events(&thread).await, "ui_catalog"), 2);

    // a viewer that connects later reads the history as it was: version 1 until the thread was
    // shown version 2, then version 2 to the end (the third job's older catalog changed nothing)
    let viewer = chat
        .agui_connect(&thread, None, true)
        .await
        .collect_frames(WAIT)
        .await;
    let versions: Vec<i64> = snapshot_catalogs(&viewer)
        .iter()
        .map(|c| c["version"].as_i64().unwrap())
        .collect();
    assert_eq!(versions.first(), Some(&1));
    assert_eq!(versions.last(), Some(&2));
    assert!(versions.windows(2).all(|w| w[0] <= w[1]), "{versions:?}");
    assert_eq!(chat.thread(&thread).await["state"], "done");
}

fn count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|e| e["kind"] == kind).count()
}

/// Waits until the thread is `done` in its `jobs`-th job (the log has `jobs - 1` boundaries).
async fn wait_for_jobs(chat: &Chat, thread: &str, jobs: usize) {
    orch_testsupport::eventually("the job to finish", || async {
        let events = chat.events(thread).await;
        let done = events
            .iter()
            .filter(|e| e["kind"] == "thread_state" && e["data"]["state"] == "done")
            .count();
        (done == jobs).then_some(())
    })
    .await;
}

async fn a_catalog_that_breaks_a_rule_is_refused_before_anything_is_written(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let mut forged = ui_catalog(1);
    forged["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
    let thread = Uuid::now_v7().to_string();
    let body = Chat::agui_input(
        &thread,
        "run-1",
        &[("m-1", "echo hi")],
        json!({"forwardedProps": {"vymalo.uiCatalog": forged}}),
    );
    assert_eq!(chat.agui_post("plain", &body).await.status().as_u16(), 400);

    let mut big = ui_catalog(1);
    big["catalog"]["components"]["Text"]["description"] = json!("x".repeat(70_000));
    let body = Chat::agui_input(
        &thread,
        "run-1",
        &[("m-1", "echo hi")],
        json!({"forwardedProps": {"vymalo.uiCatalog": big}}),
    );
    assert_eq!(chat.agui_post("plain", &body).await.status().as_u16(), 413);

    // no thread, and the agent was never asked
    assert_eq!(chat.get(&format!("/api/threads/{thread}")).await.0, 404);
    assert!(world.plain.executions().is_empty());
}

// ---- the extension: what the agent is told (ADR 0023, ui-catalog/v1) ------------------------------

/// A world whose `plain` agent lists A2UI (taking catalogs inline) and `ui-catalog/v1`.
async fn world_with_the_extension(backend: Backend) -> World {
    World::with(
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
    .await
}

async fn two_screens_of_different_versions_reach_the_agent_as_the_extension_says(backend: Backend) {
    let world = world_with_the_extension(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    let (v1, v2) = (ui_catalog(1), ui_catalog(2));

    // the first message of the context: the agent gets version 1 inline
    run(&chat, &thread, "run-1", "echo one", with_ui_catalog(1)).await;
    wait_for_jobs(&chat, &thread, 1).await;
    // a newer screen opens the chat: version 2 inline
    run(&chat, &thread, "run-2", "echo two", with_ui_catalog(2)).await;
    wait_for_jobs(&chat, &thread, 2).await;
    // an older screen joins with version 1: the agent is told the newest, with no catalog
    run(&chat, &thread, "run-3", "echo three", with_ui_catalog(1)).await;
    wait_for_jobs(&chat, &thread, 3).await;
    // a message that carries none: the same reference
    run(&chat, &thread, "run-4", "echo four", json!({})).await;
    wait_for_jobs(&chat, &thread, 4).await;

    let calls = world.plain.executions();
    assert_eq!(calls.len(), 4);
    let told = |call: &orch_testsupport::Call, v: &Value, inline: bool| {
        // the agent's A2A server holds metadata numbers as doubles: `2` arrives as `2.0`
        let mut seen = call.ui_catalog.clone().unwrap();
        seen["version"] = json!(seen["version"].as_f64().unwrap() as u64);
        assert_eq!(
            seen,
            json!({"catalogId": v["catalogId"], "version": v["version"],
                   "digest": v["digest"], "inline": inline})
        );
        assert!(call.activates(UI_CATALOG_EXTENSION));
    };
    told(&calls[0], &v1, true);
    let inline = |call: &orch_testsupport::Call| -> Vec<Value> {
        call.inline_catalogs.iter().map(integral_numbers).collect()
    };
    assert_eq!(inline(&calls[0]), vec![v1["catalog"].clone()]);
    told(&calls[1], &v2, true);
    assert_eq!(inline(&calls[1]), vec![v2["catalog"].clone()]);
    told(&calls[2], &v2, false);
    assert!(
        calls[2].inline_catalogs.is_empty(),
        "the older screen's catalog is not sent"
    );
    told(&calls[3], &v2, false);
    assert!(calls[3].inline_catalogs.is_empty());
    // our catalog is listed first in every message, whether or not it is inline
    for call in &calls {
        let ids = &call.a2ui_capabilities.as_ref().unwrap()["v0.9.1"]["supportedCatalogIds"];
        assert_eq!(ids[0], UI_CATALOG_ID);
    }
}

async fn a_card_without_the_extension_is_sent_no_catalog_though_the_log_holds_it(backend: Backend) {
    // `plain` of the default world lists neither A2UI nor the extension
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();

    run(&chat, &thread, "run-1", "echo one", with_ui_catalog(1)).await;
    wait_for_jobs(&chat, &thread, 1).await;

    assert_eq!(
        count(&chat.events(&thread).await, "ui_catalog"),
        1,
        "the log records it"
    );
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].ui_catalog, None);
    assert!(calls[0].inline_catalogs.is_empty());
    assert_eq!(calls[0].a2ui_capabilities, None, "plain A2A, as before");
    assert!(!calls[0].activates(UI_CATALOG_EXTENSION));
}

async fn the_capabilities_document_lists_the_extensions_the_card_lists(backend: Backend) {
    let world = world_with_the_extension(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let (status, doc) = chat.agui_capabilities("plain").await;
    assert_eq!(status, 200);
    assert_eq!(doc["custom"][UI_CATALOG_EXTENSION], json!({}));
    assert!(doc["custom"].get(THREAD_TOOLS_EXTENSION).is_none());
    // the card is read live: an agent that drops it is not flagged on the next read
    world.plain.set_extensions(&[]);
    let (_, doc) = chat.agui_capabilities("plain").await;
    assert!(doc["custom"].get(UI_CATALOG_EXTENSION).is_none());
}

/// The catalog the web ships (`web/src/features/chat/lib/a2ui/catalog/`): `catalog.json` and the
/// lock that says its version and digest, as `ThreadAgent` sends them.
fn the_web_catalog() -> (Value, Value) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../web/src/features/chat/lib/a2ui/catalog");
    let read = |name: &str| -> Value {
        let text = std::fs::read_to_string(dir.join(name)).unwrap();
        serde_json::from_str(&text).unwrap()
    };
    let catalog = read("catalog.json");
    let lock = read("catalog.lock.json");
    let sent = json!({
        "catalogId": catalog["catalogId"],
        "version": lock["version"],
        "digest": lock["digest"],
        "catalog": catalog,
    });
    (sent, lock)
}

async fn the_catalog_the_web_ships_is_accepted_as_it_sends_it(backend: Backend) {
    let (sent, lock) = the_web_catalog();
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();

    // the web's own lock digest is the digest the orchestrator computes over catalog.json (the two
    // canonical JSONs agree), every component schema is JSON Schema, and each names itself
    let extra = json!({"forwardedProps": {"vymalo.uiCatalog": sent}});
    let frames = run(&chat, &thread, "run-1", "echo hi", extra).await;
    assert_eq!(frames[0].event["type"], "RUN_STARTED", "{frames:?}");
    let named = json!({"catalogId": sent["catalogId"], "version": lock["version"], "digest": lock["digest"]});
    let snapshots = snapshot_catalogs(&frames);
    assert!(!snapshots.is_empty());
    assert!(snapshots.iter().all(|c| *c == named), "{snapshots:?}");
    chat.wait_state(&thread, "done").await;

    // what the log holds is what the web sent, whole, and its snapshot field is the one the web reads
    let events = chat.events(&thread).await;
    assert_eq!(events[0]["kind"], "ui_catalog");
    assert_eq!(events[0]["data"], sent);
    let thread_info = chat.thread(&thread).await;
    assert_eq!(thread_info["state"], "done");
    let viewer = chat
        .agui_connect(&thread, None, true)
        .await
        .collect_frames(WAIT)
        .await;
    assert!(snapshot_catalogs(&viewer).iter().all(|c| *c == named));
}

backends!(
    the_catalog_the_web_ships_is_accepted_as_it_sends_it,
    the_screens_catalog_is_recorded_once_per_digest_and_the_snapshot_names_the_newest,
    a_catalog_that_breaks_a_rule_is_refused_before_anything_is_written,
    two_screens_of_different_versions_reach_the_agent_as_the_extension_says,
    a_card_without_the_extension_is_sent_no_catalog_though_the_log_holds_it,
    the_capabilities_document_lists_the_extensions_the_card_lists,
);
