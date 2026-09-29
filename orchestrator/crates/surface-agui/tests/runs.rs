//! A run over HTTP: what the requester is shown, from the first event its input caused to the
//! terminal event of that run.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_ports::memory::Call;
use serde_json::{Value, json};
use support::*;

fn outcome(frames: &[Frame]) -> Value {
    let last = frames.last().unwrap();
    match last.kind() {
        "RUN_FINISHED" => last.event["outcome"].clone(),
        "RUN_ERROR" => json!({"type": "error", "code": last.event["code"]}),
        other => panic!("the stream ends in {other}"),
    }
}

#[tokio::test]
async fn a_new_thread_streams_its_run_and_ends_after_the_terminal_event() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = input(&thread, "run-a", &[("msg-1", "echo hello")]);

    let resp = h.post("plain", Some(ALICE), &body).await;
    assert_eq!(resp.status().as_u16(), 200);
    let headers = resp.headers().clone();
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(headers["cache-control"], "no-cache, no-transform");
    assert_eq!(headers["x-accel-buffering"], "no");
    let frames = Stream::new(resp).all().await;

    // The requester holds its own message: no triad for it.
    assert_eq!(
        kinds(&frames),
        [
            "RUN_STARTED",
            "STATE_SNAPSHOT",
            "SUBAGENT_STARTED",
            "ACTIVITY_SNAPSHOT",
            "STATE_SNAPSHOT",
            "ACTIVITY_SNAPSHOT",
            "ACTIVITY_SNAPSHOT",
            "SUBAGENT_FINISHED",
            "STATE_SNAPSHOT",
            "RUN_FINISHED",
        ]
    );
    let first = &frames[0].event;
    assert_eq!(first["threadId"], thread.as_str());
    assert_eq!(first["runId"], "run-a", "the run keeps the consumer's id");
    assert_eq!(first["protocolVersion"], "1.0");
    assert_eq!(frames[1].event["snapshot"]["thread"]["state"], "queued");
    assert_eq!(
        frames[1].event["snapshot"]["thread"]["target"],
        json!({"agentId": "plain"})
    );
    assert_eq!(outcome(&frames), json!({"type": "success"}));

    // What the agent produced is in the stream, and `id:` is the log's seq on resume points.
    let artifact = frames
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.artifact")
        .unwrap();
    assert_eq!(artifact.event["content"]["text"], "echo: echo hello");
    let ids: Vec<i64> = frames.iter().filter_map(|f| f.id).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    assert_eq!(*ids.last().unwrap(), 5, "the log has five events");

    // The consumer's id is the thread's id; the message and run ids are in the log.
    let events = h.events(ALICE, &thread).await;
    assert_eq!(events.len(), 5);
    assert_eq!(events[0]["threadId"], thread.as_str());
    assert_eq!(events[0]["actor"]["name"], ALICE);
    assert_eq!(events[0]["data"]["text"], "echo hello");
    assert_eq!(events[0]["data"]["messageId"], "msg-1");
    assert_eq!(events[0]["data"]["runId"], "run-a");
    assert_eq!(h.thread(ALICE, &thread).await["title"], "echo hello");
    assert_eq!(h.agent.sends().len(), 1, "one message reached the agent");
}

#[tokio::test]
async fn a_question_ends_the_run_in_an_interrupt_and_resume_answers_it() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let first = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m-1", "ask about branches")]),
        )
        .await
        .all()
        .await;
    let interrupt = outcome(&first);
    assert_eq!(interrupt["type"], "interrupt");
    let asked = &interrupt["interrupts"][0];
    assert_eq!(asked["reason"], "input_required");
    assert_eq!(asked["message"], "Which branch?");
    let interrupt_id = asked["id"].as_str().unwrap().to_owned();
    assert_eq!(interrupt_id, "int-3");
    assert_eq!(h.state(ALICE, &thread).await, "blocked");

    // The answer: the transcript the consumer holds, plus `resume`.
    let second = h
        .run(
            "plain",
            ALICE,
            &input_with(
                &thread,
                "run-2",
                &[("m-1", "ask about branches")],
                json!({"resume": [{
                    "interruptId": interrupt_id,
                    "status": "resolved",
                    "payload": {"text": "main"},
                }]}),
            ),
        )
        .await
        .all()
        .await;
    assert_eq!(second[0].kind(), "RUN_STARTED");
    assert_eq!(second[0].event["runId"], "run-2");
    assert_eq!(outcome(&second), json!({"type": "success"}));
    // The answer had no message id of the consumer's, so the requester is shown it.
    let answer: Vec<&Frame> = second
        .iter()
        .filter(|f| f.kind() == "TEXT_MESSAGE_CONTENT")
        .collect();
    assert_eq!(answer.len(), 1);
    assert_eq!(answer[0].event["delta"], "main");

    // Same A2A task, continued.
    let sends = h.agent.sends();
    assert_eq!(sends.len(), 2);
    let (
        Call::Send { task_id: t1, .. },
        Call::Send {
            task_id: t2, text, ..
        },
    ) = (&sends[0], &sends[1])
    else {
        panic!("sends");
    };
    assert_eq!(t1, &None);
    assert!(t2.is_some(), "the second send continues the task");
    assert_eq!(text, "main");

    let events = h.events(ALICE, &thread).await;
    let answers: Vec<&Value> = events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .collect();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[1]["data"]["runId"], "run-2");
    assert!(answers[1]["data"].get("messageId").is_none());
}

#[tokio::test]
async fn a_new_message_on_a_blocked_thread_without_resume_is_the_answer() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run("plain", ALICE, &input(&thread, "r1", &[("m1", "ask me")]))
        .await
        .all()
        .await;
    let second = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "r2", &[("m1", "ask me"), ("m2", "main")]),
        )
        .await
        .all()
        .await;
    assert_eq!(outcome(&second), json!({"type": "success"}));
    // m2 came in the request, so the requester is not sent it back.
    assert!(!kinds(&second).contains(&"TEXT_MESSAGE_CONTENT"));
    let events = h.events(ALICE, &thread).await;
    assert_eq!(events[4]["data"]["messageId"], "m2");
}

#[tokio::test]
async fn a_failed_task_ends_the_run_in_run_error() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-f", &[("m", "failed please")]),
        )
        .await
        .all()
        .await;
    let last = frames.last().unwrap();
    assert_eq!(last.kind(), "RUN_ERROR");
    assert_eq!(last.event["code"], "agent_failed");
    assert_eq!(last.event["message"], "scripted failure");
    assert!(kinds(&frames).contains(&"SUBAGENT_ERROR"));
    assert_eq!(h.state(ALICE, &thread).await, "failed");
}

#[tokio::test]
async fn cancelling_a_running_thread_ends_the_run_cancelled() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let mut stream = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-c", &[("m", "slow work")]),
        )
        .await;
    // The run is under way: the first frames arrive before it ends.
    let started = stream.next(T).await.unwrap();
    assert_eq!(started.kind(), "RUN_STARTED");
    h.wait_state(ALICE, &thread, "working").await;

    let cancel = h
        .post_empty(&format!("/api/threads/{thread}/cancel"), ALICE)
        .await;
    assert_eq!(cancel.status, 202);

    let mut rest = Vec::new();
    while let Some(f) = stream.next(T).await {
        rest.push(f);
    }
    assert!(stream.ended());
    let last = rest.last().unwrap();
    assert_eq!(last.kind(), "RUN_FINISHED");
    assert_eq!(last.event["outcome"], json!({"type": "cancelled"}));
    assert!(
        rest.iter().all(|f| f.kind() != "RUN_ERROR"),
        "neither success nor failure"
    );
    assert_eq!(h.state(ALICE, &thread).await, "cancelled");
}

#[tokio::test]
async fn resume_cancelled_cancels_a_blocked_thread() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run("plain", ALICE, &input(&thread, "r1", &[("m1", "ask me")]))
        .await
        .all()
        .await;
    let frames = h
        .run(
            "plain",
            ALICE,
            &input_with(
                &thread,
                "r2",
                &[("m1", "ask me")],
                json!({"resume": [{"interruptId": "int-3", "status": "cancelled"}]}),
            ),
        )
        .await
        .all()
        .await;
    assert_eq!(frames[0].kind(), "RUN_STARTED");
    assert_eq!(outcome(&frames), json!({"type": "cancelled"}));
    assert_eq!(h.state(ALICE, &thread).await, "cancelled");
}

#[tokio::test]
async fn dropping_the_response_does_not_cancel_the_run() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let mut stream = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-d", &[("m", "gate now")]),
        )
        .await;
    assert_eq!(stream.next(T).await.unwrap().kind(), "RUN_STARTED");
    drop(stream);
    h.wait_state(ALICE, &thread, "working").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.state(ALICE, &thread).await, "working", "not cancelled");
    h.agent.release_gate();
    h.wait_state(ALICE, &thread, "done").await;
}

#[tokio::test]
async fn an_idle_run_stream_is_kept_alive() {
    let h = Harness::start().await; // keepalive configured to 150 ms
    let thread = new_thread_id();
    let mut stream = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-k", &[("m", "gate now")]),
        )
        .await;
    // Wait through a few keepalive intervals while the agent holds the run.
    while stream.next(Duration::from_millis(700)).await.is_some() {}
    assert!(
        stream.comments >= 2,
        "keepalive comments: {}",
        stream.comments
    );
    assert!(!stream.ended());
    h.agent.release_gate();
    let mut last = None;
    while let Some(f) = stream.next(T).await {
        last = Some(f);
    }
    assert_eq!(last.unwrap().kind(), "RUN_FINISHED");
    assert!(stream.ended());
}

#[tokio::test]
async fn the_release_comes_from_forwarded_props_and_reaches_the_agent() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let uri = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";
    let frames = h
        .run(
            "coder",
            ALICE,
            &input_with(
                &thread,
                "run-r",
                &[("m", "echo ship it")],
                json!({"forwardedProps": {uri: {"release": "staging"}}}),
            ),
        )
        .await
        .all()
        .await;
    assert_eq!(
        frames[1].event["snapshot"]["thread"]["target"],
        json!({"agentId": "coder", "release": "staging"})
    );
    assert_eq!(outcome(&frames), json!({"type": "success"}));
    let Call::Send { release, .. } = &h.agent.sends()[0] else {
        panic!("send");
    };
    assert_eq!(release.as_deref(), Some("staging"));
    assert_eq!(
        h.thread(ALICE, &thread).await["target"]["release"],
        "staging"
    );
}

#[tokio::test]
async fn an_unknown_release_is_refused_before_the_stream_and_creates_nothing() {
    let h = Harness::start().await;
    let uri = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";
    for (agent, release) in [("coder", "nightly"), ("plain", "staging")] {
        let thread = new_thread_id();
        let r = h
            .refused(
                agent,
                Some(ALICE),
                &input_with(
                    &thread,
                    "run-x",
                    &[("m", "echo hi")],
                    json!({"forwardedProps": {uri: {"release": release}}}),
                ),
            )
            .await;
        r.problem(400);
        let none = h.get(&format!("/api/threads/{thread}"), Some(ALICE)).await;
        assert_eq!(none.status, 404, "{agent}/{release}: no thread was created");
    }
}

#[tokio::test]
async fn frontend_tools_context_and_state_are_ignored_with_a_warning_not_refused() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input_with(
                &thread,
                "run-w",
                &[("m", "echo hi")],
                json!({
                    "tools": [{"name": "t", "description": "d", "parameters": {"type": "object"}}],
                    "context": [{"description": "c", "value": "v"}],
                    "state": {"anything": 1},
                    "parentRunId": "run-0",
                    "protocolVersion": "1.3",
                    "unknownMember": true,
                }),
            ),
        )
        .await
        .all()
        .await;
    assert_eq!(outcome(&frames), json!({"type": "success"}));
}

#[tokio::test]
async fn a_large_transcript_fits_the_body_limit_of_the_route() {
    // The resource API's limit is 1 MiB; a client that re-sends a long history needs more.
    let h = Harness::start().await;
    let thread = new_thread_id();
    let big = "x".repeat(2 * 1024 * 1024);
    let body = input_with(
        &thread,
        "run-b",
        &[("m", "echo hi")],
        json!({"state": {"blob": big}}),
    );
    let frames = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(outcome(&frames), json!({"type": "success"}));
}
