//! An agent that answers through an artifact (kagent, ADR 0031 amendment of 2026-10-07), end to end:
//! the fake agent streams what kagent's Go runtime streams (its reply as the words of `working`
//! statuses, then **one unnamed text artifact**, then `completed` with no message), and the
//! thread's log holds the text once as the agent's answer, no artifact, the run ends done and the
//! chat's projection says the answer. On the in-memory store and on Postgres.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;

async fn the_text_of_an_unnamed_artifact_is_recorded_once_as_the_agents_answer(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let id = chat
        .create_thread("plain", "artifact-answer kagent says hello", None)
        .await;
    chat.wait_state(&id, "done").await;

    let events = chat.events(&id).await;
    let said = data_of(&events, "agent_message");
    assert_eq!(said.len(), 1, "{:?}", shape(&events));
    assert_eq!(said[0]["text"], "kagent says hello");
    assert_eq!(said[0]["final"], true);
    assert_eq!(said[0]["purpose"], "answer");
    assert!(
        events.iter().all(|e| e["kind"] != "artifact"),
        "the text is the answer, not also an artifact: {:?}",
        shape(&events)
    );
    // the answer is in the log before the completion that ends the turn
    let at = |kind: &str| events.iter().position(|e| e["kind"] == kind).unwrap();
    assert!(
        at("agent_message") < at("thread_state"),
        "{:?}",
        shape(&events)
    );
    assert_contiguous(&events);

    // the chat's projection: the assistant's message says it once
    let mut viewer = chat.agui_connect(&id, None, false).await;
    let frames = viewer
        .frames_until(Duration::from_secs(30), |f| {
            f.event["type"] == "RUN_FINISHED"
        })
        .await;
    let assistant: Vec<&str> = frames
        .iter()
        .filter(|f| f.event["type"] == "TEXT_MESSAGE_START" && f.event["role"] == "assistant")
        .filter_map(|f| f.event["messageId"].as_str())
        .collect();
    let words: Vec<&str> = frames
        .iter()
        .filter(|f| {
            f.event["type"] == "TEXT_MESSAGE_CONTENT"
                && f.event["messageId"]
                    .as_str()
                    .is_some_and(|m| assistant.contains(&m))
        })
        .filter_map(|f| f.event["delta"].as_str())
        .collect();
    assert_eq!(
        words,
        ["kagent says hello"],
        "the assistant says the answer once"
    );
    assert_eq!(
        world.plain.executions().len(),
        1,
        "one message reached the agent"
    );
}

backends!(the_text_of_an_unnamed_artifact_is_recorded_once_as_the_agents_answer);
