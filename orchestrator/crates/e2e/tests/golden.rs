//! Golden transcripts: what the real orchestrator emits for each scripted agent behaviour,
//! written to `docs/api/examples/*.events.json` and replayed by the web's
//! `src/chat/golden.test.ts`, so the chat surface is checked against the orchestrator's real
//! event sequences (kinds, status spellings, failure shape, message finality), not only
//! against the schema in `docs/api/chat-api.yaml`.
//!
//! `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test golden` rewrites the files; without it any
//! difference fails the test.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::path::PathBuf;

use common::*;
use serde_json::{Value, json};

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/examples")
}

/// Ids and clocks are not part of the story: threads, timestamps and agent message ids become
/// placeholders. `seq`, actors and every `data` field stay as emitted.
fn normalise(events: Vec<Value>) -> Value {
    Value::Array(
        events
            .into_iter()
            .map(|mut e| {
                e["threadId"] = json!("<thread-id>");
                e["at"] = json!("<timestamp>");
                if e["kind"] == "agent_message" {
                    e["data"]["messageId"] = json!("<message-id>");
                }
                e
            })
            .collect(),
    )
}

/// One scripted run to its final state; returns the thread's events.
async fn run(world: &World, name: &str) -> Vec<Value> {
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let (id, last) = match name {
        "echo" => (chat.create_thread("plain", "echo hi", None).await, "done"),
        "ask" => {
            let id = chat
                .create_thread("plain", "ask about branches", None)
                .await;
            chat.wait_state(&id, "blocked").await;
            let (status, body) = chat.post_message(&id, "main").await;
            assert_eq!(status, 202, "{body}");
            (id, "done")
        }
        "cancel" => {
            let id = chat.create_thread("plain", "slow work", None).await;
            chat.wait_state(&id, "working").await;
            assert_eq!(chat.cancel(&id).await, 202);
            (id, "cancelled")
        }
        "fail" => (
            chat.create_thread("plain", "fail please", None).await,
            "failed",
        ),
        "talk" => (
            chat.create_thread("plain", "talk to me", None).await,
            "done",
        ),
        "release" => (
            chat.create_thread("coder", "echo ship it", Some("staging"))
                .await,
            "done",
        ),
        other => panic!("unknown scenario {other}"),
    };
    chat.wait_state(&id, last).await;
    chat.events(&id).await
}

const SCENARIOS: [&str; 6] = ["echo", "ask", "cancel", "fail", "talk", "release"];

#[tokio::test]
async fn transcripts_match_docs_api_examples() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let dir = examples_dir();
    let mut stale = Vec::new();
    for name in SCENARIOS {
        let world = World::start(Backend::Memory).await;
        let got = normalise(run(&world, name).await);
        let path = dir.join(format!("{name}.events.json"));
        let mut text = serde_json::to_string_pretty(&got).unwrap();
        text.push('\n');
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &text).unwrap();
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(want) if want == text => {}
            Ok(want) => stale.push(format!(
                "{}: differs\n--- want\n{want}--- got\n{text}",
                path.display()
            )),
            Err(e) => stale.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        stale.is_empty(),
        "golden transcripts are out of date; run `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test golden` and review the diff:\n{}",
        stale.join("\n")
    );
}

async fn an_agent_that_talks_shows_its_status_text_and_message_once(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "talk to me", None).await;
    chat.wait_state(&id, "done").await;
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:working",
            "agent_message",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&events);
    assert_eq!(events[2]["data"]["detail"], "Reading the repository");
    let message = &events[3];
    assert_eq!(message["actor"]["type"], "agent");
    assert_eq!(message["actor"]["name"], "plain");
    assert_eq!(message["data"]["text"], "Plan: add a test");
    assert_eq!(
        message["data"]["final"], true,
        "the adapter reports agent messages as final"
    );
    assert!(
        message["data"]["messageId"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(data_of(&events, "agent_message").len(), 1);
}

backends!(an_agent_that_talks_shows_its_status_text_and_message_once);
