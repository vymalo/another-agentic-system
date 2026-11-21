//! A stream ends after the deployment's lifetime, always, and a client that reopens it with the
//! last `id:` it saw is shown every later event exactly once (ADR 0082).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_api::ApiConfig;
use orch_auth_header::HeaderAuth;
use support::*;

/// The resume points of `frames`, in the order they were written.
fn ids(frames: &[Frame]) -> Vec<i64> {
    frames.iter().filter_map(|f| f.id).collect()
}

#[tokio::test]
async fn a_stream_ends_at_its_lifetime_and_a_reopen_with_its_cursor_says_each_later_event_once() {
    // The identity header carries no `expires_at`: that is the credential whose stream used to be
    // unbounded (ADR 0033 section 6), and the case ADR 0082 closes.
    let api = ApiConfig {
        sse_keepalive: Duration::from_millis(100),
        sse_lifetime: Duration::from_millis(500),
        ..ApiConfig::default()
    };
    let h = Harness::start_with(api, HeaderAuth::new()).await;
    let thread = new_thread_id();
    // A run that parks until the gate is released: the first stream ends at its lifetime, while
    // the run is still open, so its end is the lifetime and not a run's.
    let _run = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-a", &[("msg-1", "gate hold")]),
        )
        .await;
    h.wait_state(ALICE, &thread, "working").await;

    let first = h.connect(&thread, ALICE, None).await;
    // `all` waits for the end and asserts the server closed the stream: it is the lifetime, since
    // the run below is still open when it happens.
    let seen = first.all().await;
    assert_eq!(
        h.state(ALICE, &thread).await,
        "working",
        "the run was still open when the stream ended"
    );
    let first_ids = ids(&seen);
    assert!(
        !first_ids.is_empty(),
        "the replay has resume points: {seen:?}"
    );
    let cursor = *first_ids.last().unwrap();

    // The answer arrives after the stream is gone: ending a stream loses nothing.
    h.agent.release_gate();
    h.wait_state(ALICE, &thread, "done").await;

    // The client reopens from where it stopped.
    let mut second = h.connect(&thread, ALICE, Some(cursor)).await;
    let rest = second.until(|f| f.kind() == "RUN_FINISHED").await;
    let rest_ids = ids(&rest);
    assert!(
        rest_ids.iter().all(|id| *id > cursor),
        "the reopen says what follows the cursor: {rest_ids:?} after {cursor}"
    );
    // Exactly once: the cursor is where the log goes on, so nothing is said twice and nothing of
    // what the client already holds comes back.
    let mut joined = first_ids;
    joined.extend(rest_ids);
    let mut sorted = joined.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, joined, "every event once, in the log's order");
    assert_eq!(
        rest.last().unwrap().kind(),
        "RUN_FINISHED",
        "the answer of the run the client was waiting for"
    );
    // The message the client sent itself is not echoed back to it.
    assert!(
        !rest.iter().any(|f| f.event["messageId"] == "msg-1"),
        "{rest:?}"
    );
}
