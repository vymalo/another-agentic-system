//! SSE resume with `Last-Event-ID`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;

const WAIT: Duration = Duration::from_secs(20);

async fn last_event_id_n_replays_exactly_n_plus_one_onwards(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "echo resume", None).await;
    chat.wait_state(&id, "done").await;
    let all = chat.events(&id).await;
    assert_eq!(all.len(), 5);

    for n in 0..=5_i64 {
        let mut sse = chat.stream(&id, Some(n)).await;
        let mut seqs = Vec::new();
        while let Some((seq, _, data)) = sse.next_event(Duration::from_millis(400)).await {
            assert_eq!(data["seq"], seq, "the SSE id is the event's seq");
            seqs.push(seq);
        }
        let want: Vec<i64> = ((n + 1)..=5).collect();
        assert_eq!(seqs, want, "Last-Event-ID: {n}");
    }
}

async fn a_live_stream_resumed_mid_run_delivers_each_later_event_once(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "gate live", None).await;
    chat.wait_events(&id, 2).await; // user_message, agent_status(working)

    let mut sse = chat.stream(&id, Some(1)).await;
    let (seq, kind, _) = sse.next_event(WAIT).await.expect("first replayed event");
    assert_eq!((seq, kind.as_str()), (2, "agent_status"));

    world.plain.release_gate();
    let rest = sse
        .collect_until(WAIT, |kind, _| kind == "thread_state")
        .await;
    let seqs: Vec<i64> = rest.iter().map(|(s, _, _)| *s).collect();
    assert_eq!(seqs, [3, 4, 5]);
    assert!(
        sse.next_event(Duration::from_millis(400)).await.is_none(),
        "nothing more"
    );
}

async fn a_client_that_reconnects_keeps_a_gapless_view(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "gate flaky", None).await;

    // First connection sees the start, then drops.
    let mut first = chat.stream(&id, None).await;
    let mut seen = Vec::new();
    for _ in 0..2 {
        let (seq, _, _) = first.next_event(WAIT).await.expect("event");
        seen.push(seq);
    }
    drop(first);
    world.plain.release_gate();
    chat.wait_state(&id, "done").await;

    let mut second = chat.stream(&id, seen.last().copied()).await;
    while let Some((seq, _, _)) = second.next_event(Duration::from_millis(400)).await {
        seen.push(seq);
    }
    assert_eq!(seen, [1, 2, 3, 4, 5]);
}

/// A `Last-Event-ID` past the end of the log (a stale id, e.g. from another thread's stream)
/// replays nothing, but the stream is not dead: what happens next arrives.
async fn a_last_event_id_beyond_the_end_replays_nothing_then_goes_live(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "gate beyond", None).await;
    chat.wait_events(&id, 2).await; // user_message, agent_status(working)

    let mut sse = chat.stream(&id, Some(999)).await;
    assert_eq!(sse.status, 200);
    assert!(
        sse.next_event(Duration::from_millis(400)).await.is_none(),
        "nothing exists after seq 999"
    );
    world.plain.release_gate();
    let rest = sse
        .collect_until(WAIT, |kind, _| kind == "thread_state")
        .await;
    let seqs: Vec<i64> = rest.iter().map(|(s, _, _)| *s).collect();
    assert_eq!(seqs, [3, 4, 5], "only what happened after the connection");
}

backends!(
    a_last_event_id_beyond_the_end_replays_nothing_then_goes_live,
    last_event_id_n_replays_exactly_n_plus_one_onwards,
    a_live_stream_resumed_mid_run_delivers_each_later_event_once,
    a_client_that_reconnects_keeps_a_gapless_view,
);
