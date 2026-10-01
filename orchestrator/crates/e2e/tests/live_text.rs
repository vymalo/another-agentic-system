//! Live text through the whole stack (ADR 0027, MVP slice 6): an agent whose card lists
//! `text-stream/v1` streams its reply, the orchestrator activates the extension and **relays** the
//! pieces on the wakeup port (on Postgres: `NOTIFY` over every process's listener), and a viewer
//! connected to **another replica** than the one that holds the agent's stream sees the words
//! grow, then the log's message completes them. The log holds the reply once. On the in-memory
//! store and on Postgres.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_core::{STEPS_EXTENSION, TEXT_STREAM_EXTENSION};
use orch_testsupport::{Frame, stream_id, stream_text};
use serde_json::{Value, json};

const WITHIN: Duration = Duration::from_secs(30);

async fn world_streaming_on(backend: Backend, extensions: &[&str]) -> World {
    World::with(
        backend,
        Setup {
            plain: orch_testsupport::FakeAgentOptions {
                extensions: extensions.iter().map(|u| (*u).to_owned()).collect(),
                ..orch_testsupport::FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

fn live(frame: &Frame) -> Option<&Value> {
    frame.event.get("metadata")?.get("vymalo.live")
}

fn kind(frame: &Frame) -> &str {
    frame.event["type"].as_str().unwrap()
}

/// What the message `id` says, read the way the web does (a live delta continues from its `offset`
/// in UTF-16 code units).
fn reading(frames: &[Frame], id: &str) -> String {
    let mut text = String::new();
    for f in frames
        .iter()
        .filter(|f| kind(f) == "TEXT_MESSAGE_CONTENT" && f.event["messageId"] == id)
    {
        if let Some(offset) = live(f)
            .and_then(|l| l.get("offset"))
            .and_then(Value::as_u64)
        {
            let mut units = 0;
            text = text
                .chars()
                .take_while(|c| {
                    let keep = units < offset as usize;
                    units += c.len_utf16();
                    keep
                })
                .collect();
        }
        text.push_str(f.event["delta"].as_str().unwrap());
    }
    text
}

fn starts(frames: &[Frame], id: &str) -> usize {
    frames
        .iter()
        .filter(|f| kind(f) == "TEXT_MESSAGE_START" && f.event["messageId"] == id)
        .count()
}

/// The `agent_message` events of the log: `(messageId, text, final)`.
fn messages(events: &[Value]) -> Vec<(String, String, bool)> {
    events
        .iter()
        .filter(|e| e["kind"] == "agent_message")
        .map(|e| {
            (
                e["data"]["messageId"].as_str().unwrap().to_owned(),
                e["data"]["text"].as_str().unwrap().to_owned(),
                e["data"]["final"].as_bool().unwrap(),
            )
        })
        .collect()
}

async fn a_reply_streamed_by_a_worker_is_seen_growing_by_a_viewer_on_another_replica(
    backend: Backend,
) {
    let world = world_streaming_on(backend, &[TEXT_STREAM_EXTENSION]).await;
    // Two processes on one database: this one serves the viewer, that one holds the agent's stream.
    let serving = world.instance_with("serving", false).await;
    let worker = world.instance("worker").await;
    let chat = world.chat(&serving);
    let id = chat.create_thread("plain", "stream tell me", None).await;

    let mut viewer = chat.agui_connect(&id, None, false).await;
    let frames = viewer
        .frames_until(WITHIN, |f| kind(f) == "RUN_FINISHED")
        .await;
    chat.wait_state(&id, "done").await;

    // The orchestrator asked for the stream, and the agent sent it
    let call = world.plain.executions().pop().unwrap();
    assert!(call.activates_text_stream(), "{call:?}");
    let task = &call.task_id;
    let reply = stream_id(task);

    // The viewer saw the reply grow: one live message, several live deltas before its end, none of
    // them a resume point, and the log's message to complete it.
    assert_eq!(starts(&frames, &reply), 1, "{frames:?}");
    let start = frames
        .iter()
        .position(|f| kind(f) == "TEXT_MESSAGE_START" && f.event["messageId"] == reply.as_str())
        .unwrap();
    assert_eq!(live(&frames[start]), Some(&json!({})));
    let end = frames
        .iter()
        .position(|f| kind(f) == "TEXT_MESSAGE_END" && f.event["messageId"] == reply.as_str())
        .unwrap();
    let grown: Vec<&Frame> = frames[start..end]
        .iter()
        .filter(|f| {
            kind(f) == "TEXT_MESSAGE_CONTENT"
                && f.event["messageId"] == reply.as_str()
                && live(f).is_some_and(|l| l.get("final").is_none())
        })
        .collect();
    assert!(
        grown.len() >= 2,
        "the words arrived piece by piece, not at once: {} live deltas",
        grown.len()
    );
    assert!(
        frames[start..end].iter().all(|f| f.id.is_none()
            || !(kind(f) == "TEXT_MESSAGE_CONTENT" && f.event["messageId"] == reply.as_str())),
        "a live delta is never a resume point"
    );
    assert_eq!(live(&frames[end]), Some(&json!({"final": true})));
    assert!(
        frames[end].id.is_some(),
        "the log's message keeps its resume point"
    );
    assert_eq!(reading(&frames, &reply), stream_text());

    // The log has the reply once, whole and final, under the stream's id; nothing partial.
    let events = chat.events(&id).await;
    assert_eq!(
        messages(&events),
        [(reply.clone(), stream_text(), true)],
        "{:?}",
        shape(&events)
    );
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_message",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&events);
    serving.shutdown().await;
    worker.shutdown().await;
}

async fn a_viewer_that_connects_mid_stream_is_told_the_text_so_far_and_reads_the_reply_once(
    backend: Backend,
) {
    let world = world_streaming_on(backend, &[TEXT_STREAM_EXTENSION]).await;
    let serving = world.instance_with("serving", false).await;
    let worker = world.instance("worker").await;
    let chat = world.chat(&serving);
    let id = chat.create_thread("plain", "stream tell me", None).await;
    // The agent has begun and has said a few pieces (about 150 ms apart)
    eventually("the first words are out", || async {
        (!world.plain.executions().is_empty()).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(450)).await;

    let mut viewer = chat.agui_connect(&id, None, false).await;
    let frames = viewer
        .frames_until(WITHIN, |f| kind(f) == "RUN_FINISHED")
        .await;
    let call = world.plain.executions().pop().unwrap();
    let reply = stream_id(&call.task_id);
    // Whatever it missed it was told from the start (the refresh) or by the log: the reply is read
    // once, whole, under one message.
    assert_eq!(starts(&frames, &reply), 1, "{frames:?}");
    assert_eq!(reading(&frames, &reply), stream_text());
    serving.shutdown().await;
    worker.shutdown().await;
}

async fn the_words_before_a_tool_call_are_a_message_of_their_own(backend: Backend) {
    let world = world_streaming_on(backend, &[TEXT_STREAM_EXTENSION, STEPS_EXTENSION]).await;
    let serving = world.instance_with("serving", false).await;
    let worker = world.instance("worker").await;
    let chat = world.chat(&serving);
    let id = chat.create_thread("plain", "stream-words go", None).await;
    let mut viewer = chat.agui_connect(&id, None, false).await;
    let frames = viewer
        .frames_until(WITHIN, |f| kind(f) == "RUN_FINISHED")
        .await;
    chat.wait_state(&id, "done").await;
    let task = world.plain.executions().pop().unwrap().task_id;
    let (words, reply) = (format!("{task}-words"), stream_id(&task));

    let events = chat.events(&id).await;
    assert_eq!(
        messages(&events),
        [
            (
                words.clone(),
                "Let me run the tests first.".to_owned(),
                true
            ),
            (reply.clone(), stream_text(), true)
        ]
    );
    // The words come before the tool call and the answer after it, both final
    let order: Vec<&str> = events
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .filter(|k| matches!(*k, "agent_message" | "agent_step"))
        .collect();
    assert_eq!(order, ["agent_message", "agent_step", "agent_message"]);

    // Each is one message on the screen, whole
    for (id, text) in [
        (words.as_str(), "Let me run the tests first."),
        (reply.as_str(), stream_text().as_str()),
    ] {
        assert_eq!(starts(&frames, id), 1, "{id}");
        assert_eq!(reading(&frames, id), text, "{id}");
    }
    serving.shutdown().await;
    worker.shutdown().await;
}

async fn a_reply_that_is_given_up_ends_on_the_screen_and_logs_nothing(backend: Backend) {
    let world = world_streaming_on(backend, &[TEXT_STREAM_EXTENSION]).await;
    let serving = world.instance_with("serving", false).await;
    let worker = world.instance("worker").await;
    let chat = world.chat(&serving);
    let id = chat.create_thread("plain", "stream-abandon go", None).await;
    let mut viewer = chat.agui_connect(&id, None, false).await;
    let frames = viewer
        .frames_until(WITHIN, |f| matches!(kind(f), "RUN_FINISHED" | "RUN_ERROR"))
        .await;
    chat.wait_state(&id, "failed").await;
    let task = world.plain.executions().pop().unwrap().task_id;
    let reply = stream_id(&task);
    let end = frames
        .iter()
        .find(|f| kind(f) == "TEXT_MESSAGE_END" && f.event["messageId"] == reply.as_str())
        .expect("the live message ended");
    assert_eq!(live(end), Some(&json!({"abandoned": true})));
    assert!(messages(&chat.events(&id).await).is_empty());
    serving.shutdown().await;
    worker.shutdown().await;
}

async fn an_agent_that_does_not_list_the_extension_is_not_asked_and_its_reply_is_read_all_the_same(
    backend: Backend,
) {
    let world = world_streaming_on(backend, &[]).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "stream tell me", None).await;
    chat.wait_state(&id, "done").await;
    let call = world.plain.executions().pop().unwrap();
    assert!(!call.activates(TEXT_STREAM_EXTENSION));
    // The response is data: the reply is in the log, once, under the stream's id
    let events = chat.events(&id).await;
    assert_eq!(
        messages(&events),
        [(stream_id(&call.task_id), stream_text(), true)]
    );
    orch.shutdown().await;
}

async fn an_agent_that_never_streamed_still_has_its_reply_logged(backend: Backend) {
    let world = world_streaming_on(backend, &[TEXT_STREAM_EXTENSION]).await;
    let serving = world.instance_with("serving", false).await;
    let worker = world.instance("worker").await;
    let chat = world.chat(&serving);
    let id = chat.create_thread("plain", "stream-marker go", None).await;
    chat.wait_state(&id, "done").await;
    let task = world.plain.executions().pop().unwrap().task_id;
    // no chunk was ever sent: a viewer reads the plain message, with no live frame
    let frames = chat
        .agui_connect(&id, None, true)
        .await
        .collect_frames(WITHIN)
        .await;
    assert!(frames.iter().all(|f| live(f).is_none()));
    let reply = stream_id(&task);
    assert_eq!(starts(&frames, &reply), 1);
    assert_eq!(reading(&frames, &reply), stream_text());
    assert_eq!(
        messages(&chat.events(&id).await),
        [(reply, stream_text(), true)]
    );
    serving.shutdown().await;
    worker.shutdown().await;
}

/// The worker dies with the reply half written; the log gets the whole text once all the same,
/// from the status the agent ends the turn with (read by the replica that takes over).
async fn a_worker_that_dies_mid_stream_loses_nothing(backend: Backend) {
    let world = world_streaming_on(backend, &[TEXT_STREAM_EXTENSION]).await;
    let first = world.instance("orch-1").await;
    let chat = world.chat(&first);
    let id = chat.create_thread("plain", "stream tell me", None).await;
    eventually("the agent executes the task", || async {
        (world.plain.executions().len() == 1).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    first.kill();

    let second = world.instance("orch-2").await;
    let chat = world.chat(&second);
    chat.wait_state(&id, "done").await;
    assert_eq!(world.plain.rpc_count("send_streaming_message"), 1);
    let task = world.plain.executions().pop().unwrap().task_id;
    let events = chat.events(&id).await;
    assert_eq!(
        messages(&events),
        [(stream_id(&task), stream_text(), true)],
        "the whole text, once: {:?}",
        shape(&events)
    );
    assert_contiguous(&events);
    second.shutdown().await;
}

backends!(
    a_reply_streamed_by_a_worker_is_seen_growing_by_a_viewer_on_another_replica,
    a_viewer_that_connects_mid_stream_is_told_the_text_so_far_and_reads_the_reply_once,
    the_words_before_a_tool_call_are_a_message_of_their_own,
    a_reply_that_is_given_up_ends_on_the_screen_and_logs_nothing,
    an_agent_that_does_not_list_the_extension_is_not_asked_and_its_reply_is_read_all_the_same,
    an_agent_that_never_streamed_still_has_its_reply_logged,
    a_worker_that_dies_mid_stream_loses_nothing,
);
