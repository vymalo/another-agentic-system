//! Everything refused before the stream: RFC 9457 problems, nothing streamed, nothing written.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use serde_json::{Value, json};
use support::*;

async fn no_thread(h: &Harness, user: &str, id: &str) {
    let r = h.get(&format!("/api/threads/{id}"), Some(user)).await;
    assert_eq!(r.status, 404, "the refused request created a thread");
}

#[tokio::test]
async fn the_edge_identity_is_required() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = input(&thread, "r", &[("m", "echo hi")]);
    let r = h.refused("plain", None, &body).await;
    r.problem(401);
    no_thread(&h, ALICE, &thread).await;
    // A malformed identity is refused too.
    let resp = h
        .client
        .post(h.url("/agui/agents/plain"))
        .header("X-Auth-Request-Email", "nobody")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    assert!(h.agent.sends().is_empty());
}

#[tokio::test]
async fn a_dev_user_serves_a_request_without_the_header() {
    let h = Harness::start_with(dev_config(Some("dev@example.com"))).await;
    let thread = new_thread_id();
    let resp = h
        .post("plain", None, &input(&thread, "r", &[("m", "echo hi")]))
        .await;
    assert_eq!(resp.status().as_u16(), 200);
    let frames = Stream::new(resp).all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    assert_eq!(h.thread("dev@example.com", &thread).await["state"], "done");
}

#[tokio::test]
async fn a_body_that_is_not_a_run_input_is_a_400() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let post = |body: &'static str| {
        let h = &h;
        async move {
            resp_of(
                h.client
                    .post(h.url("/agui/agents/plain"))
                    .header("X-Auth-Request-Email", ALICE)
                    .header("Content-Type", "application/json")
                    .body(body)
                    .send()
                    .await
                    .unwrap(),
            )
            .await
        }
    };
    post("not json").await.problem(400);
    post("").await.problem(400);
    post("[]").await.problem(400);
    post("{}").await.problem(400);
    post(r#"{"threadId": "t", "runId": "r"}"#)
        .await
        .problem(400);
    // An unknown role is malformed, not something to skip.
    let bad_role = json!({"threadId": thread, "runId": "r",
        "messages": [{"id": "m", "role": "wizard", "content": "hi"}]});
    h.refused("plain", Some(ALICE), &bad_role)
        .await
        .problem(400);
    no_thread(&h, ALICE, &thread).await;
}

#[tokio::test]
async fn the_media_types_are_json_in_and_event_stream_out() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = input(&thread, "r", &[("m", "echo hi")]).to_string();
    // text/plain would need no CORS preflight from another origin.
    let r = resp_of(
        h.client
            .post(h.url("/agui/agents/plain"))
            .header("X-Auth-Request-Email", ALICE)
            .header("Content-Type", "text/plain")
            .body(body.clone())
            .send()
            .await
            .unwrap(),
    )
    .await;
    r.problem(415);
    let r = resp_of(
        h.client
            .post(h.url("/agui/agents/plain"))
            .header("X-Auth-Request-Email", ALICE)
            .body(body.clone())
            .send()
            .await
            .unwrap(),
    )
    .await;
    r.problem(415);
    // The optional protobuf framing is not offered.
    let r = resp_of(
        h.client
            .post(h.url("/agui/agents/plain"))
            .header("X-Auth-Request-Email", ALICE)
            .header("Content-Type", "application/json")
            .header("Accept", "application/vnd.ag-ui.event+proto")
            .body(body.clone())
            .send()
            .await
            .unwrap(),
    )
    .await;
    r.problem(406);
    no_thread(&h, ALICE, &thread).await;
    // Neither header is needed by a client that says nothing about what it accepts.
    let resp = h
        .client
        .post(h.url("/agui/agents/plain"))
        .header("X-Auth-Request-Email", ALICE)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

#[tokio::test]
async fn the_body_is_bounded_and_so_are_the_ids() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let huge = "x".repeat(orch_surface_agui::MAX_BODY_BYTES + 1);
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &input_with(&thread, "r", &[("m", "echo hi")], json!({"state": huge})),
        )
        .await;
    r.problem(413);
    let long = "i".repeat(257);
    h.refused(
        "plain",
        Some(ALICE),
        &input(&thread, &long, &[("m", "echo hi")]),
    )
    .await
    .problem(400);
    h.refused(
        "plain",
        Some(ALICE),
        &input(&thread, "r", &[(&long, "echo hi")]),
    )
    .await
    .problem(400);
    no_thread(&h, ALICE, &thread).await;
}

#[tokio::test]
async fn a_thread_id_that_is_not_a_uuid_is_a_400() {
    let h = Harness::start().await;
    for bad in [
        "not-a-uuid",
        "12345",
        "",
        "00000000-0000-7000-8000-00000000000g",
    ] {
        let r = h
            .refused("plain", Some(ALICE), &input(bad, "r", &[("m", "echo hi")]))
            .await;
        let p = r.problem(400);
        assert!(p["detail"].as_str().unwrap().contains("UUID"), "{p}");
    }
    assert!(h.agent.sends().is_empty());
}

#[tokio::test]
async fn another_protocol_major_is_a_400() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    for bad in ["2.0", "0.9", "1", "one.two"] {
        let r = h
            .refused(
                "plain",
                Some(ALICE),
                &input_with(
                    &thread,
                    "r",
                    &[("m", "echo hi")],
                    json!({"protocolVersion": bad}),
                ),
            )
            .await;
        r.problem(400);
    }
    no_thread(&h, ALICE, &thread).await;
}

#[tokio::test]
async fn an_unknown_agent_is_a_404() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let r = h
        .refused(
            "nobody",
            Some(ALICE),
            &input(&thread, "r", &[("m", "echo hi")]),
        )
        .await;
    let p = r.problem(404);
    assert!(p["detail"].as_str().unwrap().contains("agent"), "{p}");
    no_thread(&h, ALICE, &thread).await;
}

#[tokio::test]
async fn someone_elses_thread_id_is_a_404_and_the_thread_is_untouched() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let bobs = h
        .run(
            "plain",
            BOB,
            &input(&thread, "run-b", &[("mb", "ask about it")]),
        )
        .await
        .all()
        .await;
    assert_eq!(bobs.last().unwrap().kind(), "RUN_FINISHED");
    let before = h.events(BOB, &thread).await;

    // Alice mints the same id (a collision, or a guess): the same answer as for a thread that
    // does not exist for her; nothing says it exists for Bob.
    for body in [
        input(&thread, "run-a", &[("ma", "echo mine")]),
        input(&thread, "run-b", &[("mb", "ask about it")]),
        input_with(
            &thread,
            "run-c",
            &[],
            json!({"resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"text": "x"}}]}),
        ),
    ] {
        let r = h.refused("plain", Some(ALICE), &body).await;
        let p = r.problem(404);
        assert!(!p.to_string().contains(BOB), "{p}");
    }
    let r = h.get(&format!("/api/threads/{thread}"), Some(ALICE)).await;
    assert_eq!(r.status, 404);
    assert_eq!(
        h.events(BOB, &thread).await,
        before,
        "Bob's log is unchanged"
    );
    assert_eq!(h.agent.sends().len(), 1, "Alice's request reached no agent");
}

#[tokio::test]
async fn the_url_agent_must_be_the_threads_agent() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run("plain", ALICE, &input(&thread, "r1", &[("m1", "ask me")]))
        .await
        .all()
        .await;
    let r = h
        .refused(
            "coder",
            Some(ALICE),
            &input(&thread, "r2", &[("m1", "ask me"), ("m2", "main")]),
        )
        .await;
    let p = r.problem(409);
    assert!(p["detail"].as_str().unwrap().contains("plain"), "{p}");
    // Attaching is refused the same way: the thread is not that agent's.
    h.refused(
        "coder",
        Some(ALICE),
        &input(&thread, "r1", &[("m1", "ask me")]),
    )
    .await
    .problem(409);
    assert_eq!(
        h.events(ALICE, &thread).await.len(),
        4,
        "nothing was written"
    );
}

#[tokio::test]
async fn a_second_run_while_one_is_open_is_a_409() {
    let h = Harness::start().await;
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
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &input(&thread, "r2", &[("m1", "gate wait"), ("m2", "hurry")]),
        )
        .await;
    let p = r.problem(409);
    assert!(p["detail"].as_str().unwrap().contains("run"), "{p}");
    assert_eq!(
        h.events(ALICE, &thread)
            .await
            .iter()
            .filter(|e| e["kind"] == "user_message")
            .count(),
        1
    );
    h.agent.release_gate();
    open.all().await;
}

#[tokio::test]
async fn a_finished_thread_takes_the_next_message_and_can_still_be_attached_to() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let first = input(&thread, "r1", &[("m1", "echo done")]);
    h.run("plain", ALICE, &first).await.all().await;
    h.wait_state(ALICE, &thread, "done").await;
    // A message starts the thread's next job (ADR 0020): it is served, as a run of its own.
    let second = input(&thread, "r2", &[("m1", "echo done"), ("m2", "more")]);
    let frames = h.run("plain", ALICE, &second).await.all().await;
    assert_eq!(frames.first().map(|f| f.kind()), Some("RUN_STARTED"));
    assert_eq!(frames.last().map(|f| f.kind()), Some("RUN_FINISHED"));
    assert!(
        frames
            .iter()
            .any(|f| f.event["activityType"] == "vymalo.job" && f.event["content"]["job"] == 2),
        "the job starts in the run"
    );
    h.wait_state(ALICE, &thread, "done").await;
    let kinds: Vec<String> = h
        .events(ALICE, &thread)
        .await
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds.iter().filter(|k| *k == "job_started").count(), 1);
    // A retry of the same message is an attach, not a third job.
    let again = h.run("plain", ALICE, &second).await.all().await;
    assert_eq!(again.len(), frames.len());
    let kinds_after = h.events(ALICE, &thread).await.len();
    assert_eq!(kinds_after, kinds.len());
    // The first run is still there to attach to.
    assert_eq!(h.run("plain", ALICE, &first).await.all().await.len(), 10);
}

#[tokio::test]
async fn what_the_orchestrator_owns_cannot_be_sent() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    // Two new messages: one at a time.
    h.refused(
        "plain",
        Some(ALICE),
        &input(&thread, "r", &[("a", "echo 1"), ("b", "echo 2")]),
    )
    .await
    .problem(422);
    // A history the orchestrator never saw.
    let history = json!({"threadId": thread, "runId": "r", "messages": [
        {"id": "x", "role": "assistant", "content": "hello"},
        {"id": "a", "role": "user", "content": "echo 1"},
    ]});
    h.refused("plain", Some(ALICE), &history).await.problem(422);
    // Nothing to run: no message, no resume, a run that never was.
    h.refused("plain", Some(ALICE), &input(&thread, "r", &[]))
        .await
        .problem(422);
    // A message without text.
    h.refused("plain", Some(ALICE), &input(&thread, "r", &[("a", "   ")]))
        .await
        .problem(422);
    no_thread(&h, ALICE, &thread).await;
    assert!(h.agent.sends().is_empty());
}

#[tokio::test]
async fn nothing_new_on_a_known_thread_under_an_unknown_run_is_a_422() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run("plain", ALICE, &input(&thread, "r1", &[("m1", "ask me")]))
        .await
        .all()
        .await;
    h.refused(
        "plain",
        Some(ALICE),
        &input(&thread, "never", &[("m1", "ask me")]),
    )
    .await
    .problem(422);
}

#[tokio::test]
async fn a_run_id_is_never_reused_for_new_input() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run("plain", ALICE, &input(&thread, "r1", &[("m1", "ask me")]))
        .await
        .all()
        .await;
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &input(&thread, "r1", &[("m1", "ask me"), ("m2", "main")]),
        )
        .await;
    let p = r.problem(422);
    assert!(p["detail"].as_str().unwrap().contains("r1"), "{p}");
    assert_eq!(
        h.state(ALICE, &thread).await,
        "blocked",
        "the thread waits still"
    );
}

#[tokio::test]
async fn a_resume_that_answers_nothing_is_ignored_and_a_malformed_answer_is_a_422() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let first = input(&thread, "r1", &[("m1", "ask me")]);
    h.run("plain", ALICE, &first).await.all().await;
    // An id that is not open: ignored with a warning; with no message either, nothing to run.
    let stale = input_with(
        &thread,
        "r2",
        &[("m1", "ask me")],
        json!({"resume": [{"interruptId": "int-99", "status": "resolved", "payload": {"text": "x"}}]}),
    );
    h.refused("plain", Some(ALICE), &stale).await.problem(422);
    // A payload without text cannot answer a question.
    let empty = input_with(
        &thread,
        "r3",
        &[("m1", "ask me")],
        json!({"resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"choice": 1}}]}),
    );
    h.refused("plain", Some(ALICE), &empty).await.problem(422);
    // Both a resume answer and a new message: which one is the answer?
    let both = input_with(
        &thread,
        "r4",
        &[("m1", "ask me"), ("m2", "other")],
        json!({"resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"text": "x"}}]}),
    );
    h.refused("plain", Some(ALICE), &both).await.problem(422);
    assert_eq!(h.state(ALICE, &thread).await, "blocked");
}

#[tokio::test]
async fn an_agent_that_is_down_is_a_502_before_the_stream() {
    let h = Harness::start().await;
    h.agent.set_card_down("coder", true);
    let thread = new_thread_id();
    let uri = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";
    let r = h
        .refused(
            "coder",
            Some(ALICE),
            &input_with(
                &thread,
                "r",
                &[("m", "echo hi")],
                json!({"forwardedProps": {uri: {"release": "staging"}}}),
            ),
        )
        .await;
    r.problem(502);
    no_thread(&h, ALICE, &thread).await;
}

#[tokio::test]
async fn every_refusal_is_a_problem_with_a_detail_and_no_stream() {
    // The status of a problem is the response's status, and the type is about:blank.
    let h = Harness::start().await;
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &input("nope", "r", &[("m", "echo hi")]),
        )
        .await;
    let p: Value = r.problem(400);
    assert_eq!(p["type"], "about:blank");
    assert_eq!(p["title"], "Bad Request");
}

/// The ids `start_job` derives (UUID version 8, ADR 0019) are reserved: nobody can create a
/// thread under an id another user's `start_job` will need. A job that exists is still served.
#[tokio::test]
async fn a_new_thread_cannot_take_a_reserved_version_8_id_but_an_existing_job_is_served() {
    use orch_app::{Inbound, NewThread};
    use orch_core::{AgentId, AgentTarget, ThreadId, UserId};

    let h = Harness::start().await;
    let v8 = "0190aaaa-0000-8000-8000-000000000001";
    let r = h
        .refused("plain", Some(ALICE), &input(v8, "r", &[("m", "echo hi")]))
        .await;
    let problem = r.problem(400);
    assert!(
        problem["detail"].as_str().unwrap().contains("reserved"),
        "{problem}"
    );
    no_thread(&h, ALICE, v8).await;
    assert!(h.agent.sends().is_empty());
    // Versions 4 and 7 are as free as ever.
    for ok in [
        "0190aaaa-0000-7000-8000-000000000002",
        "0190aaaa-0000-4000-8000-000000000003",
    ] {
        let resp = h
            .post("plain", Some(ALICE), &input(ok, "r", &[("m", "echo hi")]))
            .await;
        assert_eq!(resp.status().as_u16(), 200, "{ok}");
        Stream::new(resp).all().await;
    }

    // A job the MCP surface started (a version 8 id) can be continued from the chat by its owner.
    let job = ThreadId(v8.parse().unwrap());
    h.app
        .create_thread_as(
            &UserId::new(ALICE),
            job,
            NewThread {
                title: None,
                target: AgentTarget {
                    agent_id: AgentId::new("plain"),
                    release: None,
                },
                text: "ask which branch".to_owned(),
            },
            Inbound::default(),
        )
        .await
        .unwrap();
    h.wait_state(ALICE, v8, "blocked").await;
    let resp = h
        .post("plain", Some(ALICE), &input(v8, "r2", &[("m2", "more")]))
        .await;
    let status = resp.status().as_u16();
    assert_eq!(status, 200, "{}", resp.text().await.unwrap());
    // Someone else cannot see it, whatever the id looks like.
    let r = h
        .refused("plain", Some(BOB), &input(v8, "r3", &[("m3", "hi")]))
        .await;
    r.problem(404);
}
