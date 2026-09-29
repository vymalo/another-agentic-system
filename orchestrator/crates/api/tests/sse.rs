//! SSE: replay-then-live, resume with Last-Event-ID, keepalive.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use support::*;

const T: Duration = Duration::from_secs(5);

async fn seqs(sse: &mut SseClient, n: usize) -> Vec<i64> {
    let mut out = Vec::new();
    for _ in 0..n {
        out.push(sse.next_event(T).await.expect("event").0);
    }
    out
}

#[tokio::test]
async fn resuming_after_last_event_id_replays_exactly_the_rest() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "echo resume").await;
    h.wait_state(ALICE, &id, "done").await;

    let mut sse = h.stream(ALICE, &id, Some("2")).await;
    assert_eq!(seqs(&mut sse, 3).await, [3, 4, 5]);
    // Exactly those: nothing else follows.
    assert!(sse.next_event(Duration::from_millis(500)).await.is_none());

    // Resume at the end: nothing to replay; from the start (0 or no header): everything.
    let mut sse = h.stream(ALICE, &id, Some("5")).await;
    assert!(sse.next_event(Duration::from_millis(300)).await.is_none());
    let mut sse = h.stream(ALICE, &id, Some("0")).await;
    assert_eq!(seqs(&mut sse, 5).await, [1, 2, 3, 4, 5]);
    let mut sse = h.stream(ALICE, &id, None).await;
    assert_eq!(seqs(&mut sse, 5).await, [1, 2, 3, 4, 5]);
    // Garbage and negative ids mean "from the start".
    for bad in ["abc", "-4", ""] {
        let mut sse = h.stream(ALICE, &id, Some(bad)).await;
        assert_eq!(seqs(&mut sse, 5).await, [1, 2, 3, 4, 5], "{bad:?}");
    }
    // Ahead of the log: nothing until something happens.
    let mut sse = h.stream(ALICE, &id, Some("99")).await;
    assert!(sse.next_event(Duration::from_millis(300)).await.is_none());
}

#[tokio::test]
async fn a_client_that_reconnects_mid_run_sees_every_event_once() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "gate live").await;
    h.wait_state(ALICE, &id, "working").await;

    let mut first = h.stream(ALICE, &id, None).await;
    assert_eq!(seqs(&mut first, 2).await, [1, 2]);
    drop(first);

    // Reconnect from seq 1 while the run is still going, then let it finish.
    let mut sse = h.stream(ALICE, &id, Some("1")).await;
    assert_eq!(seqs(&mut sse, 1).await, [2]);
    h.agent.release_gate();
    assert_eq!(seqs(&mut sse, 3).await, [3, 4, 5]);
    assert!(
        sse.next_event(Duration::from_millis(400)).await.is_none(),
        "no duplicates"
    );
}

#[tokio::test]
async fn live_events_arrive_as_they_happen() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "ask me").await;
    h.wait_state(ALICE, &id, "blocked").await;
    let mut sse = h.stream(ALICE, &id, None).await;
    assert_eq!(seqs(&mut sse, 4).await, [1, 2, 3, 4]);
    let r = h
        .post(
            &format!("/api/threads/{id}/messages"),
            Some(ALICE),
            serde_json::json!({"text": "main"}),
        )
        .await;
    assert_eq!(r.status, 202);
    let (seq, kind, data) = sse.next_event(T).await.unwrap();
    assert_eq!((seq, kind.as_str()), (5, "user_message"));
    assert_eq!(data["data"]["text"], "main");
    assert_eq!(seqs(&mut sse, 4).await, [6, 7, 8, 9]);
}

#[tokio::test]
async fn keepalive_comments_flow_on_an_idle_stream() {
    let h = Harness::start().await; // keepalive configured to 150 ms
    let id = h.create(ALICE, "plain", "echo idle").await;
    h.wait_state(ALICE, &id, "done").await;
    let mut sse = h.stream(ALICE, &id, Some("5")).await;
    let item = sse.next(Duration::from_secs(2)).await.expect("a keepalive");
    assert_eq!(item, Item::Comment("keepalive".into()));
    // ... and again: it is periodic.
    assert!(sse.next(Duration::from_secs(2)).await.is_some());
}

#[tokio::test]
async fn stream_headers_and_default_keepalive() {
    let h = Harness::start_with(orch_api::ApiConfig::default()).await;
    assert_eq!(
        orch_api::ApiConfig::default().sse_keepalive,
        Duration::from_secs(15)
    );
    let id = h.create(ALICE, "plain", "echo hdr").await;
    let sse = h.stream(ALICE, &id, None).await;
    assert_eq!(sse.status.as_u16(), 200);
    assert!(
        sse.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(sse.headers["cache-control"], "no-cache, no-transform");
    assert_eq!(sse.headers["x-accel-buffering"], "no");
}
