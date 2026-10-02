//! A message sent while an agent works (ADR 0036): `forwardedProps["vymalo.send"]` on a run
//! posted while another is open, over HTTP on the in-memory stack with the dispatcher running.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use serde_json::{Value, json};
use support::*;

fn send(thread: &str, run: &str, messages: &[(&str, &str)], how: Value) -> Value {
    input_with(
        thread,
        run,
        messages,
        json!({"forwardedProps": {"vymalo.send": how}}),
    )
}

/// The thread's user messages, from the log: `(text, delivery)`.
async fn user_messages(h: &Harness, thread: &str) -> Vec<(String, Option<String>)> {
    h.events(ALICE, thread)
        .await
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .map(|e| {
            (
                e["data"]["text"].as_str().unwrap().to_owned(),
                e["data"]["delivery"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn run_ids(frames: &[Frame]) -> Vec<&str> {
    frames
        .iter()
        .filter(|f| f.kind() == "RUN_STARTED")
        .map(|f| f.event["runId"].as_str().unwrap())
        .collect()
}

/// A thread whose agent is working (it waits for the gate), with the response of its first run
/// still open.
async fn working(h: &Harness) -> (String, Stream) {
    let thread = new_thread_id();
    let mut open = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "r1", &[("m1", "gate wait")]),
        )
        .await;
    assert_eq!(open.next(T).await.unwrap().kind(), "RUN_STARTED");
    h.wait_state(ALICE, &thread, "working").await;
    (thread, open)
}

#[tokio::test]
async fn a_message_that_says_steer_is_served_as_a_run_of_its_own() {
    let h = Harness::start().await;
    let (thread, open) = working(&h).await;

    let mut second = h
        .run(
            "plain",
            ALICE,
            &send(
                &thread,
                "r2",
                &[("m1", "gate wait"), ("m2", "you were wrong since line 1")],
                json!("steer"),
            ),
        )
        .await;
    // the response is a run: it starts with RUN_STARTED, under the request's id, and does not say
    // the message back to the person who sent it
    let head = second.until(|f| f.kind() == "STATE_SNAPSHOT").await;
    assert_eq!(kinds(&head), ["RUN_STARTED", "STATE_SNAPSHOT"]);
    assert_eq!(head[0].event["runId"], "r2");
    assert_eq!(head[1].event["snapshot"]["thread"]["state"], "working");

    // the run that was open is finished by the message, and its response ends there
    let first = open.all().await;
    assert_eq!(first.last().unwrap().kind(), "RUN_FINISHED");
    assert_eq!(first.last().unwrap().event["runId"], "r1");
    assert_eq!(first.last().unwrap().event["outcome"]["type"], "success");
    assert!(
        first
            .iter()
            .any(|f| f.kind() == "SUBAGENT_FINISHED" && f.event["outcome"]["type"] == "suspended"),
        "the invocation is suspended, not ended: {:?}",
        kinds(&first)
    );

    // the log says it was sent while the agent worked
    assert_eq!(
        user_messages(&h, &thread).await,
        [
            ("gate wait".to_owned(), None),
            (
                "you were wrong since line 1".to_owned(),
                Some("steer".to_owned())
            )
        ]
    );

    // the agent finishes its turn (the message reaches it after it: the dispatcher does not steer
    // yet); the second run ends with the job it was opened in
    h.agent.release_gate();
    let rest = second.through_run().await;
    assert_eq!(rest.last().unwrap().event["runId"], "r2");
    assert_eq!(rest.last().unwrap().event["outcome"]["type"], "success");
}

#[tokio::test]
async fn a_message_that_says_interrupt_stops_the_task_and_starts_the_next_job_in_its_run() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let mut open = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "r1", &[("m1", "slow work")]),
        )
        .await;
    assert_eq!(open.next(T).await.unwrap().kind(), "RUN_STARTED");
    h.wait_state(ALICE, &thread, "working").await;

    let second = h
        .run(
            "plain",
            ALICE,
            &send(
                &thread,
                "r2",
                &[("m1", "slow work"), ("m2", "echo do X instead")],
                json!("interrupt"),
            ),
        )
        .await
        .all()
        .await;
    assert_eq!(run_ids(&second), ["r2"]);
    assert_eq!(second.last().unwrap().event["outcome"]["type"], "success");
    // the cancelled task, the next job, and only then the end
    let story: Vec<String> = second
        .iter()
        .filter(|f| f.kind() == "ACTIVITY_SNAPSHOT")
        .map(|f| {
            format!(
                "{} {}",
                f.event["activityType"].as_str().unwrap(),
                f.event["content"]["status"]
                    .as_str()
                    .or(f.event["content"]["job"].as_u64().map(|_| "job").or(None))
                    .unwrap_or("")
            )
        })
        .collect();
    assert_eq!(
        story[..3],
        [
            "vymalo.status canceled",
            "vymalo.job job",
            "vymalo.status working"
        ],
        "{story:?}"
    );
    assert!(
        !second
            .iter()
            .any(|f| f.event["snapshot"]["thread"]["state"] == "cancelled"),
        "the thread is never cancelled for it"
    );

    let first = open.all().await;
    assert_eq!(first.last().unwrap().event["runId"], "r1");
    assert_eq!(first.last().unwrap().event["outcome"]["type"], "success");

    assert_eq!(
        user_messages(&h, &thread).await,
        [
            ("slow work".to_owned(), None),
            ("echo do X instead".to_owned(), Some("interrupt".to_owned()))
        ]
    );
    h.wait_state(ALICE, &thread, "done").await;
    let log = h.events(ALICE, &thread).await;
    let jobs: Vec<&Value> = log
        .iter()
        .filter(|e| e["kind"] == "job_started")
        .map(|e| &e["data"]["job"])
        .collect();
    assert_eq!(jobs, [2]);
}

#[tokio::test]
async fn without_vymalo_send_a_second_run_is_still_a_409_and_says_what_would_be_served() {
    let h = Harness::start().await;
    let (thread, open) = working(&h).await;
    for forwarded in [json!({}), json!({"vymalo.send": null})] {
        let body = input_with(
            &thread,
            "r2",
            &[("m1", "gate wait"), ("m2", "hurry")],
            json!({"forwardedProps": forwarded}),
        );
        let p = h.refused("plain", Some(ALICE), &body).await.problem(409);
        assert!(p["detail"].as_str().unwrap().contains("vymalo.send"), "{p}");
    }
    assert_eq!(user_messages(&h, &thread).await.len(), 1, "nothing written");
    h.agent.release_gate();
    open.all().await;
}

#[tokio::test]
async fn a_malformed_send_is_a_400_before_the_stream_and_nothing_is_written() {
    let h = Harness::start().await;
    let (thread, open) = working(&h).await;
    for bad in [json!("stop"), json!("Steer"), json!(true), json!(["steer"])] {
        let body = send(&thread, "r2", &[("m1", "gate wait"), ("m2", "hurry")], bad);
        let p = h.refused("plain", Some(ALICE), &body).await.problem(400);
        assert!(
            p["detail"].as_str().unwrap().contains("steer"),
            "the problem names the values: {p}"
        );
    }
    // a thread that does not exist yet reads the member as well
    let fresh = send(&new_thread_id(), "r1", &[("m1", "echo hi")], json!("stop"));
    h.refused("plain", Some(ALICE), &fresh).await.problem(400);
    assert_eq!(user_messages(&h, &thread).await.len(), 1);
    h.agent.release_gate();
    open.all().await;
}

#[tokio::test]
async fn only_a_message_is_served_while_a_run_is_open() {
    let h = Harness::start().await;
    let (thread, open) = working(&h).await;
    // two new messages: one at a time (422), as everywhere
    let two = send(
        &thread,
        "r2",
        &[("m1", "gate wait"), ("m2", "a"), ("m3", "b")],
        json!("steer"),
    );
    h.refused("plain", Some(ALICE), &two).await.problem(422);
    // nothing new under an unknown run: nothing to run
    let none = send(&thread, "r2", &[("m1", "gate wait")], json!("steer"));
    h.refused("plain", Some(ALICE), &none).await.problem(422);
    // a run id is never reused
    let reused = send(
        &thread,
        "r1",
        &[("m1", "gate wait"), ("m2", "a")],
        json!("steer"),
    );
    h.refused("plain", Some(ALICE), &reused).await.problem(422);
    // an action is not a message
    let action = input_with(
        &thread,
        "r2",
        &[],
        json!({"forwardedProps": {"vymalo.send": "steer", "a2uiAction": {"userAction": {
            "name": "go", "surfaceId": "s1", "sourceComponentId": "go", "context": {}}}}}),
    );
    let r = h.refused("plain", Some(ALICE), &action).await;
    assert!(r.status == 409 || r.status == 422, "{}", r.status);
    // someone else's thread is not there for them, whatever it says
    let bob = send(
        &thread,
        "r9",
        &[("m1", "gate wait"), ("m2", "a")],
        json!("steer"),
    );
    h.refused("plain", Some(BOB), &bob).await.problem(404);
    assert_eq!(user_messages(&h, &thread).await.len(), 1);
    h.agent.release_gate();
    open.all().await;
}

#[tokio::test]
async fn a_retried_send_attaches_to_its_run_and_is_never_written_twice() {
    let h = Harness::start().await;
    let (thread, open) = working(&h).await;
    let body = send(
        &thread,
        "r2",
        &[("m1", "gate wait"), ("m2", "steer me")],
        json!("steer"),
    );
    let mut first = h.run("plain", ALICE, &body).await;
    first.until(|f| f.kind() == "STATE_SNAPSHOT").await;
    // the same POST again (the connection dropped, and the client retries): the run it opened
    let mut again = h.run("plain", ALICE, &body).await;
    let head = again.until(|f| f.kind() == "STATE_SNAPSHOT").await;
    assert_eq!(kinds(&head), ["RUN_STARTED", "STATE_SNAPSHOT"]);
    assert_eq!(head[0].event["runId"], "r2");
    assert_eq!(user_messages(&h, &thread).await.len(), 2);
    h.agent.release_gate();
    open.all().await;
}

#[tokio::test]
async fn a_viewer_reads_the_two_runs_and_a_reconnect_after_the_message_is_consistent() {
    let h = Harness::start().await;
    let (thread, open) = working(&h).await;
    let body = send(
        &thread,
        "r2",
        &[("m1", "gate wait"), ("m2", "steer me")],
        json!("steer"),
    );
    let second = h.run("plain", ALICE, &body).await;
    open.all().await;
    h.agent.release_gate();
    second.all().await;
    h.wait_state(ALICE, &thread, "done").await;

    // a viewer that joins now reads the whole thread: two runs for the first turn, then the
    // job the message started
    let full = h.connect(&thread, ALICE, None).await.until_quiet().await;
    assert_eq!(run_ids(&full)[..2], ["r1", "r2"], "{:?}", run_ids(&full));
    // r1 ends in the frame before r2 starts, and r2 holds the message
    let r2 = full
        .iter()
        .position(|f| f.kind() == "RUN_STARTED" && f.event["runId"] == "r2");
    assert!(full[..r2.unwrap()].last().unwrap().kind() == "RUN_FINISHED");

    // from every resume point, the rest and nothing else
    for (at, frame) in full.iter().enumerate() {
        let Some(cursor) = frame.id else { continue };
        let got = h
            .connect(&thread, ALICE, Some(cursor))
            .await
            .until_quiet()
            .await;
        let want = &full[at + 1..];
        let skipped = got.len().checked_sub(want.len()).unwrap_or_else(|| {
            panic!(
                "cursor {cursor}: {:?} is shorter than {:?}",
                kinds(&got),
                kinds(want)
            )
        });
        assert!(skipped <= 3, "cursor {cursor}: a preamble of {skipped}");
        assert_eq!(&got[skipped..], want, "cursor {cursor}");
        assert!(got[..skipped].iter().all(|f| f.id.is_none()));
        if skipped > 0 {
            assert_eq!(got[0].kind(), "RUN_STARTED", "cursor {cursor}");
        }
    }
}
