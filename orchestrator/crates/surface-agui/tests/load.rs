//! The process's load on `GET /metrics`: open event streams and dispatcher slots, with no label.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::{Duration, Instant};

use support::*;

async fn gauge(h: &Harness, name: &str) -> u64 {
    let r = h.get("/metrics", None).await;
    let text = String::from_utf8(r.body).unwrap();
    assert!(!text.contains(ALICE), "{text}");
    text.lines()
        .find_map(|l| l.strip_prefix(name).and_then(|v| v.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no {name} in {text}"))
        .parse()
        .unwrap()
}

async fn until(h: &Harness, name: &str, want: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = gauge(h, name).await;
        if got == want {
            return;
        }
        assert!(Instant::now() < deadline, "{name} is {got}, want {want}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_subscriber_gauge_follows_streams_opening_and_closing() {
    let h = Harness::start().await;
    assert_eq!(gauge(&h, "sse_subscribers_open").await, 0);
    let mut threads = Vec::new();
    let mut runs = Vec::new();
    for n in 0..3 {
        let thread = new_thread_id();
        runs.push(
            h.run(
                "plain",
                ALICE,
                &input(&thread, &format!("run-{n}"), &[("msg-1", "gate hold")]),
            )
            .await,
        );
        threads.push(thread);
    }
    // The run streams above count too: drop them, then open exactly three connect streams.
    drop(runs);
    until(&h, "sse_subscribers_open", 0).await;
    let mut streams = Vec::new();
    for thread in &threads {
        streams.push(h.connect(thread, ALICE, None).await);
    }
    until(&h, "sse_subscribers_open", 3).await;
    streams.pop();
    until(&h, "sse_subscribers_open", 2).await;
    drop(streams);
    until(&h, "sse_subscribers_open", 0).await;
    h.agent.release_gate();
}

#[tokio::test]
async fn the_slots_gauges_say_the_configured_concurrency_and_what_is_busy() {
    let h = Harness::start().await;
    let configured = gauge(&h, "dispatcher_slots_configured").await;
    assert!(configured > 0, "the harness runs a dispatcher");
    assert_eq!(gauge(&h, "dispatcher_slots_in_use").await, 0);
    let thread = new_thread_id();
    let _run = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-a", &[("msg-1", "gate hold")]),
        )
        .await;
    h.wait_state(ALICE, &thread, "working").await;
    until(&h, "dispatcher_slots_in_use", 1).await;
    h.agent.release_gate();
    h.wait_state(ALICE, &thread, "done").await;
    until(&h, "dispatcher_slots_in_use", 0).await;
}
