//! Live text on the AG-UI streams (ADR 0027): the words of a reply that is still being written
//! reach a requester's response and a viewer's connect stream between the frames of the log, never
//! as a resume point, and the log's final message completes the same message. A viewer that
//! connects mid-stream is told the text so far, and reads the reply once.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_ports::memory::{STREAM_PIECES, stream_id, stream_text};
use serde_json::{Value, json};
use support::*;

/// `metadata["vymalo.live"]` of a frame, if it has it.
fn live(frame: &Frame) -> Option<&Value> {
    frame.event.get("metadata")?.get("vymalo.live")
}

/// What the message `id` says, read the way the web does (a delta continues from its `offset`, in
/// UTF-16 code units, when it has one).
fn reading(frames: &[Frame], id: &str) -> String {
    let mut text = String::new();
    for f in frames.iter().filter(|f| f.event["messageId"] == id) {
        if f.kind() != "TEXT_MESSAGE_CONTENT" {
            continue;
        }
        if let Some(offset) = live(f)
            .and_then(|l| l.get("offset"))
            .and_then(Value::as_u64)
        {
            let kept: String = {
                let mut units = 0;
                text.chars()
                    .take_while(|c| {
                        let keep = units < offset as usize;
                        units += c.len_utf16();
                        keep
                    })
                    .collect()
            };
            text = kept;
        }
        text.push_str(f.event["delta"].as_str().unwrap());
    }
    text
}

fn started(frames: &[Frame], id: &str) -> usize {
    frames
        .iter()
        .filter(|f| f.kind() == "TEXT_MESSAGE_START" && f.event["messageId"] == id)
        .count()
}

async fn gated_stream(h: &Harness) -> (String, Stream) {
    let thread = new_thread_id();
    let run = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m1", "stream-gate go")]),
        )
        .await;
    (thread, run)
}

#[tokio::test]
async fn the_requesters_response_shows_the_reply_growing_and_then_completes_it() {
    let h = Harness::start().await;
    let (_thread, mut run) = gated_stream(&h).await;
    let id = stream_id("task-1");
    let first = STREAM_PIECES[..3].concat();

    // The agent says three pieces and goes quiet: the response shows them as they come.
    let mut frames = Vec::new();
    while reading(&frames, &id) != first {
        frames.push(run.next(T).await.expect("the response stalled"));
    }
    for f in frames.iter().filter(|f| live(f).is_some()) {
        assert_eq!(f.id, None, "a live frame is never a resume point: {f:?}");
    }

    h.agent.release_gate();
    frames.extend(run.through_run().await);
    // The reply opened once as a live message, and the log's message completed it.
    assert_eq!(started(&frames, &id), 1);
    assert_eq!(reading(&frames, &id), stream_text());
    let end = frames
        .iter()
        .find(|f| f.kind() == "TEXT_MESSAGE_END" && f.event["messageId"] == id.as_str())
        .unwrap();
    assert_eq!(live(end), Some(&json!({"final": true})));
    assert!(end.id.is_some(), "the log's message keeps its resume point");
}

#[tokio::test]
async fn a_viewer_sees_live_frames_without_resume_points_and_the_final_with_one() {
    let h = Harness::start().await;
    let (thread, run) = gated_stream(&h).await;
    let id = stream_id("task-1");
    // The viewer joins once the agent has said three pieces and gone quiet.
    let mut viewer = h.connect(&thread, ALICE, None).await;
    let seen = viewer
        .until(|f| {
            f.kind() == "TEXT_MESSAGE_CONTENT"
                && f.event["messageId"] == id.as_str()
                && live(f).is_some()
        })
        .await;
    // The replay first (the user's message, the agent's start), then live text.
    assert_eq!(seen.first().unwrap().kind(), "RUN_STARTED");
    let live_start = seen
        .iter()
        .position(|f| f.kind() == "TEXT_MESSAGE_START" && f.event["messageId"] == id.as_str())
        .expect("the live message opened");
    assert_eq!(live(&seen[live_start]), Some(&json!({})));
    for f in &seen[live_start..] {
        assert_eq!(f.id, None, "a live frame is never a resume point: {f:?}");
    }

    // The refresh repeats the text so far from the start, so the viewer reads the first three
    // pieces (more than one frame may be needed).
    let first = STREAM_PIECES[..3].concat();
    let mut frames = seen;
    while reading(&frames, &id) != first {
        let f = viewer
            .next(T)
            .await
            .unwrap_or_else(|| panic!("the stream stalled; read {:?}", reading(&frames, &id)));
        assert!(f.id.is_none() || f.kind() != "TEXT_MESSAGE_CONTENT");
        frames.push(f);
    }

    h.agent.release_gate();
    let rest = viewer.through_run().await;
    frames.extend(rest);
    // Once: one START of the reply, the whole text read, completed by the log's message.
    assert_eq!(started(&frames, &id), 1);
    assert_eq!(reading(&frames, &id), stream_text());
    let end = frames
        .iter()
        .find(|f| f.kind() == "TEXT_MESSAGE_END" && f.event["messageId"] == id.as_str())
        .unwrap();
    assert_eq!(live(end), Some(&json!({"final": true})));
    assert!(end.id.is_some(), "the log's message keeps its resume point");
    // The last CONTENT of the message is the log's: the rest of the words, marked final.
    let finals: Vec<&Frame> = frames
        .iter()
        .filter(|f| {
            f.kind() == "TEXT_MESSAGE_CONTENT"
                && f.event["messageId"] == id.as_str()
                && live(f).is_some_and(|l| l.get("final") == Some(&json!(true)))
        })
        .collect();
    assert_eq!(finals.len(), 1);
    drop(run);
}

#[tokio::test]
async fn a_second_viewer_that_joins_late_reads_the_reply_once_by_the_refresh_then_the_final() {
    let h = Harness::start().await;
    let (thread, _run) = gated_stream(&h).await;
    let id = stream_id("task-1");
    let first = STREAM_PIECES[..3].concat();

    let mut a = h.connect(&thread, ALICE, None).await;
    let mut frames_a = Vec::new();
    while reading(&frames_a, &id) != first {
        frames_a.push(a.next(T).await.expect("viewer a stalled"));
    }

    // A second viewer joins while the agent is quiet: it has never heard the first pieces.
    let mut b = h.connect(&thread, ALICE, None).await;
    let mut frames_b = Vec::new();
    while reading(&frames_b, &id) != first {
        frames_b.push(b.next(T).await.expect("viewer b stalled"));
    }
    assert_eq!(started(&frames_b, &id), 1);

    h.agent.release_gate();
    frames_a.extend(a.through_run().await);
    frames_b.extend(b.through_run().await);
    for frames in [&frames_a, &frames_b] {
        assert_eq!(started(frames, &id), 1);
        assert_eq!(reading(frames, &id), stream_text());
    }
}

#[tokio::test]
async fn a_connection_that_opens_after_the_reply_is_in_the_log_sees_the_plain_message() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-1", &[("m1", "stream tell me")]),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    let id = stream_id("task-1");
    let replay = h.connect(&thread, ALICE, None).await.through_run().await;
    // No live frame at all: the log says the reply whole, as it always did.
    assert!(replay.iter().all(|f| live(f).is_none()), "{replay:?}");
    assert_eq!(started(&replay, &id), 1);
    assert_eq!(reading(&replay, &id), stream_text());
}

#[tokio::test]
async fn a_reply_that_is_given_up_ends_on_the_screen_and_is_not_in_the_log() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let posted = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m1", "stream-abandon go")]),
        )
        .await
        .all()
        .await;
    let id = stream_id("task-1");
    let ended = posted
        .iter()
        .find(|f| f.kind() == "TEXT_MESSAGE_END" && f.event["messageId"] == id.as_str())
        .expect("the live message ended");
    assert_eq!(live(ended), Some(&json!({"abandoned": true})));
    // The log has no message for it.
    let events = h.events(ALICE, &thread).await;
    assert!(events.iter().all(|e| e["kind"] != "agent_message"));
}
