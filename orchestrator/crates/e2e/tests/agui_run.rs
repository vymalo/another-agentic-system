//! The AG-UI run route against the real stack: `POST /agui/agents/{agentId}` over HTTP, the
//! dispatcher, the A2A adapter and an in-process fake A2A agent, on the in-memory store and on
//! Postgres. Every event that leaves the route is checked against the vendored AG-UI schema.
//!
//! The goldens (`docs/api/examples/agui/run-<name>.agui.json`) are the responses a consumer
//! reads for each scripted behaviour. `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test agui_run`
//! rewrites them; without it any difference fails the test.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_agui_proto::testkit::assert_json_conforms;
use orch_testsupport::{Chat, Frame, SseClient, VerifierScript, with_ui_catalog};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);

fn thread_id(n: u32) -> String {
    format!("00000000-0000-7000-8000-{n:012}")
}

fn input(thread: &str, run: &str, messages: &[(&str, &str)], extra: Value) -> Value {
    Chat::agui_input(thread, run, messages, extra)
}

/// The whole response of a run; every event conforms to the schema.
async fn read(mut sse: SseClient) -> Vec<Frame> {
    assert_eq!(sse.status, 200);
    let frames = sse.collect_frames(WAIT).await;
    for frame in &frames {
        assert_json_conforms(&frame.event);
    }
    frames
}

async fn run(chat: &Chat, agent: &str, body: &Value) -> Vec<Frame> {
    read(chat.agui_run(agent, body).await).await
}

fn last(frames: &[Frame]) -> &Value {
    &frames.last().unwrap().event
}

async fn echo(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(1);

    let frames = run(
        &chat,
        "plain",
        &input(&thread, "run-1", &[("msg-1", "echo hi")], json!({})),
    )
    .await;
    assert_eq!(frames[0].event["type"], "RUN_STARTED");
    assert_eq!(frames[0].event["runId"], "run-1");
    assert_eq!(frames[0].event["threadId"], thread.as_str());
    assert_eq!(last(&frames)["type"], "RUN_FINISHED");
    assert_eq!(last(&frames)["outcome"], json!({"type": "success"}));
    let artifact = frames
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.artifact")
        .unwrap();
    assert_eq!(artifact.event["content"]["text"], "echo: echo hi");
    assert_eq!(
        artifact.event["content"]["uri"],
        orch_testsupport::fake::PR_URL
    );

    // The thread the consumer named is an ordinary thread of the resource API.
    chat.wait_state(&thread, "done").await;
    let events = chat.events(&thread).await;
    assert_eq!(shape(&events), FIVE);
    assert_eq!(events[0]["data"]["messageId"], "msg-1");
    assert_eq!(events[0]["data"]["runId"], "run-1");
    assert_eq!(chat.thread(&thread).await["id"], thread.as_str());
    assert_eq!(world.plain.executions().len(), 1);
}

async fn ask_and_resume(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(2);

    let first = run(
        &chat,
        "plain",
        &input(
            &thread,
            "run-1",
            &[("m-1", "ask about branches")],
            json!({}),
        ),
    )
    .await;
    let outcome = &last(&first)["outcome"];
    assert_eq!(outcome["type"], "interrupt");
    let asked = &outcome["interrupts"][0];
    assert_eq!(asked["reason"], "input_required");
    assert_eq!(asked["message"], "Which branch?");

    let second = run(
        &chat,
        "plain",
        &input(
            &thread,
            "run-2",
            &[("m-1", "ask about branches")],
            json!({"resume": [{
                "interruptId": asked["id"],
                "status": "resolved",
                "payload": {"text": "main"},
            }]}),
        ),
    )
    .await;
    assert_eq!(second[0].event["runId"], "run-2");
    assert_eq!(last(&second)["outcome"], json!({"type": "success"}));
    let artifact = second
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.artifact")
        .unwrap();
    assert_eq!(artifact.event["content"]["text"], "answered: main");

    // One A2A task, continued.
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].task_id, calls[1].task_id);
    assert!(calls[1].resuming);
    assert_eq!(calls[1].text, "main");
}

async fn fail(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(3);
    let frames = run(
        &chat,
        "plain",
        &input(&thread, "run-1", &[("m-1", "fail please")], json!({})),
    )
    .await;
    assert_eq!(last(&frames)["type"], "RUN_ERROR");
    assert_eq!(last(&frames)["code"], "agent_failed");
    assert_eq!(last(&frames)["message"], "scripted failure");
    chat.wait_state(&thread, "failed").await;
}

async fn cancel(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(4);
    let sse = chat
        .agui_run(
            "plain",
            &input(&thread, "run-1", &[("m-1", "slow work")], json!({})),
        )
        .await;
    chat.wait_state(&thread, "working").await;
    assert_eq!(chat.cancel(&thread).await, 202);
    let frames = read(sse).await;
    assert_eq!(last(&frames)["type"], "RUN_FINISHED");
    assert_eq!(last(&frames)["outcome"], json!({"type": "cancelled"}));
    chat.wait_state(&thread, "cancelled").await;
    assert_eq!(world.plain.cancels().len(), 1);
}

async fn a_retried_post_attaches_and_never_duplicates(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(5);
    let body = input(&thread, "run-1", &[("m-1", "echo once")], json!({}));
    let first = run(&chat, "plain", &body).await;
    chat.wait_state(&thread, "done").await;

    let retry = run(&chat, "plain", &body).await;
    assert_eq!(retry, first);
    // Another replica answers the same.
    let other = world.instance_with("orch-2", false).await;
    let via_other = run(&world.chat(&other), "plain", &body).await;
    assert_eq!(via_other, first);

    let events = chat.events(&thread).await;
    assert_eq!(shape(&events), FIVE);
    assert_eq!(world.plain.executions().len(), 1, "delivered once");
}

async fn refusals_are_problems_before_the_stream(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let post = |chat: &Chat, agent: &'static str, body: Value| {
        let chat = chat.clone();
        async move {
            let resp = chat.agui_post(agent, &body).await;
            let status = resp.status().as_u16();
            let content_type = resp
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            assert!(
                content_type.starts_with("application/problem+json"),
                "{status}"
            );
            status
        }
    };
    let ok = input(&thread_id(6), "run-1", &[("m-1", "echo hi")], json!({}));
    let mut not_uuid = ok.clone();
    not_uuid["threadId"] = json!("nope");
    assert_eq!(post(&chat, "plain", not_uuid).await, 400);
    assert_eq!(post(&chat, "nobody", ok.clone()).await, 404);
    let two = input(
        &thread_id(6),
        "run-1",
        &[("a", "echo 1"), ("b", "echo 2")],
        json!({}),
    );
    assert_eq!(post(&chat, "plain", two).await, 422);
    let nothing = input(&thread_id(6), "run-1", &[], json!({}));
    assert_eq!(post(&chat, "plain", nothing).await, 422);

    // 409: the thread exists on another agent.
    let ran = run(&chat, "plain", &ok).await;
    assert_eq!(last(&ran)["type"], "RUN_FINISHED");
    let more = input(
        &thread_id(6),
        "run-2",
        &[("m-1", "echo hi"), ("m-2", "again")],
        json!({}),
    );
    assert_eq!(post(&chat, "coder", more.clone()).await, 409, "other agent");
    chat.wait_state(&thread_id(6), "done").await;
    // (a message on the finished thread is served, not refused: it starts the next job, ADR 0020;
    // `followup.rs` and the surface's `refusals.rs` run it)

    // 401: no identity.
    let anonymous = chat.anonymous();
    let resp = anonymous.agui_post("plain", &ok).await;
    assert_eq!(resp.status().as_u16(), 401);
    assert_eq!(
        world.plain.executions().len(),
        1,
        "only the good run reached the agent"
    );
}

async fn a_thread_id_of_someone_else_is_a_404(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let bob = alice.as_user(BOB);
    let thread = thread_id(7);
    let theirs = run(
        &bob,
        "plain",
        &input(&thread, "run-b", &[("mb", "ask about it")], json!({})),
    )
    .await;
    assert_eq!(last(&theirs)["outcome"]["type"], "interrupt");
    let before = bob.events(&thread).await;

    let resp = alice
        .agui_post(
            "plain",
            &input(&thread, "run-a", &[("ma", "echo mine")], json!({})),
        )
        .await;
    assert_eq!(resp.status().as_u16(), 404);
    let problem: Value = resp.json().await.unwrap();
    assert_eq!(problem["status"], 404);
    assert!(!problem.to_string().contains(BOB), "{problem}");
    // For Alice the thread simply is not there, and Bob's is as he left it.
    assert_eq!(alice.get(&format!("/api/threads/{thread}")).await.0, 404);
    assert_eq!(bob.events(&thread).await, before);
    assert_eq!(world.plain.executions().len(), 1);
}

async fn the_release_is_selected_through_forwarded_props(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(8);
    let uri = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";
    let frames = run(
        &chat,
        "coder",
        &input(
            &thread,
            "run-1",
            &[("m-1", "echo ship it")],
            json!({"forwardedProps": {uri: {"release": "staging"}}}),
        ),
    )
    .await;
    assert_eq!(last(&frames)["outcome"], json!({"type": "success"}));
    let started = frames
        .iter()
        .find(|f| f.event["type"] == "SUBAGENT_STARTED")
        .unwrap();
    assert_eq!(
        started.event["metadata"]["vymalo.actor"]["revision"], "coder-r51",
        "the revision the release resolved to is echoed on the agent's events"
    );
    assert_eq!(chat.thread(&thread).await["target"]["release"], "staging");

    // An unknown release is the caller's mistake, before the stream.
    let resp = chat
        .agui_post(
            "coder",
            &input(
                &thread_id(9),
                "run-1",
                &[("m-1", "echo hi")],
                json!({"forwardedProps": {uri: {"release": "nightly"}}}),
            ),
        )
        .await;
    assert_eq!(resp.status().as_u16(), 400);
    assert_eq!(
        chat.get(&format!("/api/threads/{}", thread_id(9))).await.0,
        404
    );
}

async fn a_message_from_another_producer_joins_the_same_log(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(10);
    let first = run(
        &chat,
        "plain",
        &input(
            &thread,
            "run-1",
            &[("m-1", "ask about branches")],
            json!({}),
        ),
    )
    .await;
    assert_eq!(last(&first)["outcome"]["type"], "interrupt");
    // A producer that is not AG-UI (here: the application itself, as a webhook or another
    // surface would) answers the question; the AG-UI thread carries on.
    let posted = chat.seed_message(&thread, "main").await;
    assert_eq!(posted["kind"], "user_message");
    chat.wait_state(&thread, "done").await;
    let events = chat.events(&thread).await;
    assert_eq!(events[4]["kind"], "user_message");
    assert!(
        events[4]["data"].get("runId").is_none(),
        "a producer that is not AG-UI names no run"
    );
    // A run that attaches to the first one still gets exactly that run.
    let again = run(
        &chat,
        "plain",
        &input(
            &thread,
            "run-1",
            &[("m-1", "ask about branches")],
            json!({}),
        ),
    )
    .await;
    assert_eq!(again, first);
    assert_contiguous(&events);
}

backends!(
    echo,
    ask_and_resume,
    fail,
    cancel,
    a_retried_post_attaches_and_never_duplicates,
    refusals_are_problems_before_the_stream,
    a_thread_id_of_someone_else_is_a_404,
    the_release_is_selected_through_forwarded_props,
    a_message_from_another_producer_joins_the_same_log,
);

// ---- goldens -----------------------------------------------------------------------------

/// The responses to the POSTs of one scripted scenario, in order.
async fn responses_of(world: &World, name: &str, thread: &str) -> Vec<Vec<Frame>> {
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    match name {
        "echo" => vec![
            run(
                &chat,
                "plain",
                &input(thread, "run-1", &[("msg-1", "echo hi")], json!({})),
            )
            .await,
        ],
        "ask" => {
            let first = run(
                &chat,
                "plain",
                &input(
                    thread,
                    "run-1",
                    &[("msg-1", "ask about branches")],
                    json!({}),
                ),
            )
            .await;
            let interrupt = last(&first)["outcome"]["interrupts"][0]["id"].clone();
            let second = run(
                &chat,
                "plain",
                &input(
                    thread,
                    "run-2",
                    &[("msg-1", "ask about branches")],
                    json!({"resume": [{
                        "interruptId": interrupt,
                        "status": "resolved",
                        "payload": {"text": "main"},
                    }]}),
                ),
            )
            .await;
            vec![first, second]
        }
        "fail" => vec![
            run(
                &chat,
                "plain",
                &input(thread, "run-1", &[("msg-1", "fail please")], json!({})),
            )
            .await,
        ],
        // Nested steps (ADR 0025): `plain` lists `steps/v1` (`world_for`).
        "steps" => vec![
            run(
                &chat,
                "plain",
                &input(
                    thread,
                    "run-1",
                    &[("msg-1", "steps run the tests")],
                    json!({}),
                ),
            )
            .await,
        ],
        "steps-ask" => {
            let first = run(
                &chat,
                "plain",
                &input(
                    thread,
                    "run-1",
                    &[("msg-1", "steps-ask clean the build")],
                    json!({}),
                ),
            )
            .await;
            let interrupt = last(&first)["outcome"]["interrupts"][0]["id"].clone();
            let second = run(
                &chat,
                "plain",
                &input(
                    thread,
                    "run-2",
                    &[("msg-1", "steps-ask clean the build")],
                    json!({"resume": [{
                        "interruptId": interrupt,
                        "status": "resolved",
                        "payload": {"text": "yes"},
                    }]}),
                ),
            )
            .await;
            vec![first, second]
        }
        "cancel" => {
            let sse = chat
                .agui_run(
                    "plain",
                    &input(thread, "run-1", &[("msg-1", "slow work")], json!({})),
                )
                .await;
            chat.wait_state(thread, "working").await;
            assert_eq!(chat.cancel(thread).await, 202);
            vec![read(sse).await]
        }
        "release" => {
            let uri = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";
            vec![
                run(
                    &chat,
                    "coder",
                    &input(
                        thread,
                        "run-1",
                        &[("msg-1", "echo ship it")],
                        json!({"forwardedProps": {uri: {"release": "staging"}}}),
                    ),
                )
                .await,
            ]
        }
        // The verification gate (ADR 0018): one response, one run, however many attempts.
        "verify-green" | "verify-red" => {
            let script = if name == "verify-green" {
                "verify-red-once"
            } else {
                "verify-red"
            };
            vec![
                run(
                    &chat,
                    "plain",
                    &input(
                        thread,
                        "run-1",
                        &[("msg-1", &format!("{script} fix the login"))],
                        json!({"forwardedProps": {"vymalo.gate": {"require": ["agent-checks"]}}}),
                    ),
                )
                .await,
            ]
        }
        // The verifier agent in the gate: `plain` requires it in its own entry, so the run asks
        // for nothing, and one response still covers every attempt and every verification.
        "verify-verifier-green" | "verify-verifier-red" => {
            vec![
                run(
                    &chat,
                    "plain",
                    &input(
                        thread,
                        "run-1",
                        &[("msg-1", "verify-reviewed fix the login")],
                        json!({}),
                    ),
                )
                .await,
            ]
        }
        // CI on the pushed commit (ADR 0017): the run stays open while the job waits for CI,
        // one response for both attempts, with a `vymalo.ci` card for each report.
        "ci" => {
            let node = world.node("ci-node").await;
            let inbox = node.spawn_inbox(fast_inbox(), "ci-node");
            let sse = chat
                .agui_run(
                    "plain",
                    &input(
                        thread,
                        "run-1",
                        &[("msg-1", "verify-ci fix the login")],
                        json!({"forwardedProps": {"vymalo.gate": {"require": ["ci"]}}}),
                    ),
                )
                .await;
            drive_ci(&chat, &node, thread).await;
            let frames = read(sse).await;
            inbox.shutdown().await;
            vec![frames]
        }
        // The UI's catalog (ADR 0023): the first run carries version 1, the next job version 2,
        // the third version 1 again (an older screen). No frame of its own: each response is an
        // ordinary run, and the state snapshots say which catalog is current.
        "catalog" => {
            let mut responses = Vec::new();
            for (n, (text, version)) in [("echo hi", 1), ("echo again", 2), ("echo once more", 1)]
                .into_iter()
                .enumerate()
            {
                let n = n + 1;
                responses.push(
                    run(
                        &chat,
                        "plain",
                        &input(
                            thread,
                            &format!("run-{n}"),
                            &[(&format!("msg-{n}"), text)],
                            with_ui_catalog(version),
                        ),
                    )
                    .await,
                );
                chat.wait_state(thread, "done").await;
            }
            responses
        }
        // A run on a fork (ADR 0029): the screen sends the messages it holds, which are the
        // copy's, and one more. The ids the copy has are known, so the run is accepted; the
        // response is the new run alone, and its snapshots say where the thread came from.
        "fork" => {
            run(
                &chat,
                "plain",
                &input(thread, "run-1", &[("msg-1", "echo one")], json!({})),
            )
            .await;
            chat.wait_state(thread, "done").await;
            let last_seq = chat.events(thread).await.last().unwrap()["seq"].clone();
            let fork = fork_of(thread);
            let (status, forked) = chat
                .post(
                    &format!("/api/threads/{thread}/fork"),
                    Some(json!({"after": last_seq, "id": fork})),
                )
                .await;
            assert_eq!(status, 201, "{forked}");
            vec![
                run(
                    &chat,
                    "plain",
                    &input(
                        &fork,
                        "run-2",
                        &[("msg-1", "echo one"), ("msg-2", "echo two")],
                        json!({}),
                    ),
                )
                .await,
            ]
        }
        other => panic!("unknown scenario {other}"),
    }
}

/// The world a scenario runs in.
async fn world_for(name: &str) -> World {
    match name {
        "verify-verifier-green" => {
            World::with(
                Backend::Memory,
                verified_by_reviewer(VerifierScript::FindingsThenPass),
            )
            .await
        }
        "verify-verifier-red" => {
            World::with(
                Backend::Memory,
                verified_by_reviewer(VerifierScript::AlwaysFail),
            )
            .await
        }
        "steps" | "steps-ask" => world_with_steps().await,
        _ => World::start(Backend::Memory).await,
    }
}

#[tokio::test]
async fn run_responses_match_docs_api_examples() {
    let dir = examples_dir();
    let mut stale = Vec::new();
    for (n, name) in [
        "echo",
        "ask",
        "fail",
        "cancel",
        "release",
        "verify-green",
        "verify-red",
        "verify-verifier-green",
        "verify-verifier-red",
        "ci",
        "catalog",
        "steps",
        "steps-ask",
        "fork",
    ]
    .into_iter()
    .enumerate()
    {
        let world = world_for(name).await;
        let thread = thread_id(100 + u32::try_from(n).unwrap());
        let responses = responses_of(&world, name, &thread).await;
        let text = if name == "fork" {
            render_fork(&responses, &thread)
        } else {
            render(&responses, &thread)
        };
        let path = dir.join(format!("run-{name}.agui.json"));
        stale.extend(check_golden(&path, &text));
    }
    assert!(
        stale.is_empty(),
        "run goldens are out of date; run `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test agui_run` and review the diff:\n{}",
        stale.join("\n")
    );
}
