//! Retries: a POST that repeats ids the log holds attaches to the run instead of duplicating it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use serde_json::Value;
use support::*;

fn user_messages(events: &[Value]) -> usize {
    events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .count()
}

#[tokio::test]
async fn repeating_a_finished_post_replays_the_same_run_from_its_start() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = input(&thread, "run-a", &[("msg-1", "echo again")]);
    let first = h.run("plain", ALICE, &body).await.all().await;
    h.wait_state(ALICE, &thread, "done").await;

    let retry = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(retry, first, "the same frames, with the same resume points");
    assert_eq!(retry.first().unwrap().kind(), "RUN_STARTED");
    assert_eq!(retry.last().unwrap().kind(), "RUN_FINISHED");

    let events = h.events(ALICE, &thread).await;
    assert_eq!(user_messages(&events), 1, "nothing was written twice");
    assert_eq!(events.len(), 5);
    assert_eq!(h.agent.sends().len(), 1, "nothing was delivered twice");
}

#[tokio::test]
async fn repeating_a_post_while_the_run_is_going_follows_it_to_its_end() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = input(&thread, "run-a", &[("msg-1", "gate hold")]);
    let mut first = h.run("plain", ALICE, &body).await;
    assert_eq!(first.next(T).await.unwrap().kind(), "RUN_STARTED");
    h.wait_state(ALICE, &thread, "working").await;

    // The first response is lost (the consumer never saw it end); it asks again.
    let retry = h.run("plain", ALICE, &body).await;
    h.agent.release_gate();
    let retry = retry.all().await;
    assert_eq!(retry.first().unwrap().kind(), "RUN_STARTED");
    assert_eq!(retry.first().unwrap().event["runId"], "run-a");
    assert_eq!(retry.last().unwrap().kind(), "RUN_FINISHED");
    // The original, still open, saw the same end.
    let mut last = None;
    while let Some(f) = first.next(T).await {
        last = Some(f);
    }
    assert_eq!(last.unwrap(), *retry.last().unwrap());

    let events = h.events(ALICE, &thread).await;
    assert_eq!(user_messages(&events), 1);
    assert_eq!(h.agent.sends().len(), 1);
}

#[tokio::test]
async fn an_earlier_run_can_be_attached_to_after_later_ones() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let ask = input(&thread, "run-1", &[("m1", "ask me")]);
    let first = h.run("plain", ALICE, &ask).await.all().await;
    let answer = input(&thread, "run-2", &[("m1", "ask me"), ("m2", "main")]);
    let second = h.run("plain", ALICE, &answer).await.all().await;

    // Each run alone, whatever happened after it.
    assert_eq!(h.run("plain", ALICE, &ask).await.all().await, first);
    assert_eq!(h.run("plain", ALICE, &answer).await.all().await, second);
    assert_eq!(user_messages(&h.events(ALICE, &thread).await), 2);
}

#[tokio::test]
async fn two_requests_with_the_same_ids_at_once_write_one_message() {
    let h = Harness::start().await;
    for i in 0..5 {
        let thread = new_thread_id();
        let body = input(&thread, "run-a", &[("msg-1", &format!("echo race {i}"))]);
        let (a, b) = tokio::join!(h.run("plain", ALICE, &body), h.run("plain", ALICE, &body));
        let (a, b) = tokio::join!(a.all(), b.all());
        for frames in [&a, &b] {
            assert_eq!(frames.first().unwrap().kind(), "RUN_STARTED");
            assert_eq!(frames.first().unwrap().event["runId"], "run-a");
            assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
        }
        let events = h.events(ALICE, &thread).await;
        assert_eq!(user_messages(&events), 1, "round {i}: {events:?}");
    }
    assert_eq!(h.agent.sends().len(), 5, "one delivery per thread");
}

#[tokio::test]
async fn a_retried_answer_attaches_instead_of_answering_twice() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run("plain", ALICE, &input(&thread, "r1", &[("m1", "ask me")]))
        .await
        .all()
        .await;
    let answer = input_with(
        &thread,
        "r2",
        &[("m1", "ask me")],
        serde_json::json!({"resume": [{
            "interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}
        }]}),
    );
    let first = h.run("plain", ALICE, &answer).await.all().await;
    h.wait_state(ALICE, &thread, "done").await;
    let retry = h.run("plain", ALICE, &answer).await.all().await;
    assert_eq!(retry, first);
    assert_eq!(user_messages(&h.events(ALICE, &thread).await), 2);
    assert_eq!(h.agent.sends().len(), 2);
}
