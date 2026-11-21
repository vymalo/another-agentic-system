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

#[tokio::test]
async fn a_message_id_the_log_holds_is_refused_when_it_is_sent_with_different_words_and_nothing_is_written()
 {
    // C4 (ADR 0082): the id is bound to the text it was first written with, so an edited message
    // wearing the same id is a second message, not a resend. It is refused before anything is
    // written — neither silently dropped nor attached to the old run, which would lose the new
    // words.
    let h = Harness::start().await;
    let thread = new_thread_id();
    let first = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-a", &[("msg-1", "echo one")]),
        )
        .await
        .all()
        .await;
    assert_eq!(first.first().unwrap().event["runId"], "run-a");
    h.wait_state(ALICE, &thread, "done").await;
    let before = h.events(ALICE, &thread).await;

    let refused = h
        .refused(
            "plain",
            Some(ALICE),
            &input(&thread, "run-b", &[("msg-1", "echo two")]),
        )
        .await;
    let problem = refused.problem(422);
    assert_eq!(problem["code"], "message_id_reused");
    assert!(
        problem["detail"].as_str().unwrap().contains("msg-1"),
        "the refusal names the id: {problem}"
    );

    // Nothing was written, and no run started.
    let after = h.events(ALICE, &thread).await;
    assert_eq!(after.len(), before.len(), "the log is unchanged: {after:?}");
    assert!(
        after.iter().all(|e| e["data"]["runId"] != "run-b"),
        "{after:?}"
    );
    assert_eq!(h.agent.sends().len(), 1, "the agent was not asked again");

    // The same words under the same id are still a resend: it attaches to the run that message
    // opened, so the refusal above refused the words and not the retry.
    let resent = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-c", &[("msg-1", "echo one")]),
        )
        .await
        .all()
        .await;
    assert_eq!(resent.first().unwrap().event["runId"], "run-a");
    assert_eq!(h.agent.sends().len(), 1);
}

#[tokio::test]
async fn five_sends_of_one_message_under_five_run_ids_are_one_message_one_run_and_one_charge() {
    // ADR 0082, decision 5, and C4: the message id is the client's and is reused for every send of
    // the same text, whether the transport retries or the person presses Send again. A send under a
    // run id of its own is a resend, not a new run: it is shown the run its message opened.
    let h = Harness::start().await;
    // The agent reports its model's tokens (`usage/v1`, ADR 0056): one delivery is one charge.
    h.agent
        .set_extensions("plain", &[orch_core::KnownExtension::Usage]);
    let thread = new_thread_id();
    let bodies: Vec<Value> = (0..5)
        .map(|n| input(&thread, &format!("run-{n}"), &[("msg-1", "usage")]))
        .collect();

    // The first send opens the run and answers it.
    let first = h.run("plain", ALICE, &bodies[0]).await.all().await;
    assert_eq!(first.first().unwrap().event["runId"], "run-0");
    assert_eq!(first.last().unwrap().kind(), "RUN_FINISHED");
    h.wait_state(ALICE, &thread, "done").await;

    // The four resends carry a run id of their own and each is shown the run the message opened,
    // from its `RUN_STARTED` to its `RUN_FINISHED`: the person sees their answer, not a 422.
    for body in &bodies[1..] {
        let frames = h.run("plain", ALICE, body).await.all().await;
        assert_eq!(frames.first().unwrap().kind(), "RUN_STARTED");
        assert_eq!(
            frames.first().unwrap().event["runId"],
            "run-0",
            "the run the message opened, not the one the request names"
        );
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    }

    let events = h.events(ALICE, &thread).await;
    assert_eq!(user_messages(&events), 1, "one message: {events:?}");
    // One run in the log: the `user_message` records the run id of the request that wrote it.
    let opened: Vec<Value> = events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .map(|e| e["data"]["runId"].clone())
        .collect();
    assert_eq!(opened, [serde_json::json!("run-0")]);
    // One charge: the agent reported its tokens once, for one delivery.
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "model_usage_total")
            .count(),
        1,
        "{events:?}"
    );
    assert_eq!(
        h.agent.sends().len(),
        1,
        "the agent was sent the message once"
    );
}
