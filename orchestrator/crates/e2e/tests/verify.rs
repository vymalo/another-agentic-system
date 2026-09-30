//! The verification gate end to end (ADR 0018): the AG-UI run route with a gate in
//! `forwardedProps["vymalo.gate"]`, the dispatcher, the A2A adapter and an in-process fake A2A
//! agent whose scripts report a `branch` and a `checks` artifact, on the in-memory store and on
//! Postgres.
//!
//! What is checked: the run stays open across a rework (one `RUN_STARTED`, one terminal event),
//! the agent is sent back with the findings in a **new A2A task of the same context**, the job
//! ends `done` at attempt 2 or `failed` with `checks_failed` after the last attempt, and a
//! request that would weaken the gate is a 400 before anything is written.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::*;
use orch_agui_proto::testkit::assert_json_conforms;
use orch_app::GateLayer;
use orch_core::AgentId;
use orch_testsupport::{Chat, Frame};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(30);
const COMMIT_1: &str = "0000000000000000000000000000000000000001";
const COMMIT_2: &str = "0000000000000000000000000000000000000002";

fn thread_id(n: u32) -> String {
    format!("00000000-0000-7000-8000-{:012}", 300 + n)
}

/// A run input that asks for the agent's own checks to be required.
fn gated_input(thread: &str, text: &str, gate: Value) -> Value {
    Chat::agui_input(
        thread,
        "run-1",
        &[("msg-1", text)],
        json!({"forwardedProps": {"vymalo.gate": gate}}),
    )
}

async fn read(chat: &Chat, agent: &str, body: &Value) -> Vec<Frame> {
    let mut sse = chat.agui_run(agent, body).await;
    let frames = sse.collect_frames(WAIT).await;
    for frame in &frames {
        assert_json_conforms(&frame.event);
    }
    frames
}

fn types(frames: &[Frame]) -> Vec<&str> {
    frames
        .iter()
        .map(|f| f.event["type"].as_str().unwrap())
        .collect()
}

fn activities<'a>(frames: &'a [Frame], activity_type: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|f| f.event["activityType"] == activity_type)
        .map(|f| &f.event)
        .collect()
}

/// The state and job of every `STATE_SNAPSHOT`.
fn snapshots(frames: &[Frame]) -> Vec<(String, Value)> {
    frames
        .iter()
        .filter(|f| f.event["type"] == "STATE_SNAPSHOT")
        .map(|f| {
            let snapshot = &f.event["snapshot"];
            (
                snapshot["thread"]["state"].as_str().unwrap().to_owned(),
                snapshot["job"].clone(),
            )
        })
        .collect()
}

/// The world where `coder` has `gate: {require: [agent-checks]}` in its `AGENTS_FILE` entry.
fn coder_is_gated() -> Setup {
    Setup {
        target_gates: BTreeMap::from([(
            AgentId::new("coder"),
            GateLayer::from_json(&json!({"require": ["agent-checks"]}))
                .unwrap()
                .unwrap(),
        )]),
        ..Setup::default()
    }
}

async fn a_failed_check_sends_the_agent_back_and_the_second_attempt_is_green(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(1);

    let frames = read(
        &chat,
        "plain",
        &gated_input(
            &thread,
            "verify-red-once fix the login",
            json!({"require": ["agent-checks"]}),
        ),
    )
    .await;

    // One run, however many attempts: it ends once, in success.
    let kinds = types(&frames);
    assert_eq!(kinds.iter().filter(|t| **t == "RUN_STARTED").count(), 1);
    assert_eq!(kinds.iter().filter(|t| **t == "RUN_FINISHED").count(), 1);
    assert_eq!(kinds.last(), Some(&"RUN_FINISHED"));
    assert_eq!(
        frames.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );
    // The agent finishes twice, once per attempt; each finish is a subagent that ends, not the run.
    assert_eq!(
        kinds.iter().filter(|t| **t == "SUBAGENT_STARTED").count(),
        2
    );
    assert_eq!(
        kinds.iter().filter(|t| **t == "SUBAGENT_FINISHED").count(),
        2
    );

    // The checks and the rework, as activities with ids derived from the log.
    let checks = activities(&frames, "vymalo.check");
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0]["messageId"], "check-1-agent_checks");
    assert_eq!(checks[0]["content"]["status"], "failed");
    assert_eq!(checks[0]["content"]["attempt"], 1);
    assert_eq!(
        checks[0]["content"]["findings"],
        json!(["tests::login fails: expected 200, got 500"])
    );
    assert_eq!(checks[1]["messageId"], "check-2-agent_checks");
    assert_eq!(checks[1]["content"]["status"], "passed");
    let reworks = activities(&frames, "vymalo.rework");
    assert_eq!(reworks.len(), 1);
    assert_eq!(reworks[0]["messageId"], "rework-2");
    assert_eq!(reworks[0]["content"]["attempt"], 2);
    assert_eq!(reworks[0]["content"]["maxAttempts"], 3);

    // The snapshots say where the job stands: verifying at attempt 1, queued at attempt 2,
    // verifying, then done at attempt 2 with the commit of the second attempt.
    let states: Vec<(String, u64, Option<String>)> = snapshots(&frames)
        .into_iter()
        .map(|(state, job)| {
            assert_eq!(job["gate"], json!(["agent_checks"]), "{state}");
            assert_eq!(job["maxAttempts"], 3);
            (
                state,
                job["attempt"].as_u64().unwrap(),
                job["sha"].as_str().map(str::to_owned),
            )
        })
        .collect();
    let sequence: Vec<(&str, u64)> = states.iter().map(|(s, a, _)| (s.as_str(), *a)).collect();
    assert_eq!(
        sequence,
        [
            ("queued", 1),
            ("working", 1),
            ("verifying", 1),
            ("queued", 2),
            ("working", 2),
            ("verifying", 2),
            ("done", 2),
        ]
    );
    assert_eq!(states[2].2.as_deref(), Some(COMMIT_1));
    assert_eq!(states[3].2, None, "the next attempt pushes its own commit");
    assert_eq!(states[6].2.as_deref(), Some(COMMIT_2));

    // The thread of the resource API carries the job.
    chat.wait_state(&thread, "done").await;
    let record = chat.thread(&thread).await;
    assert_eq!(
        record["job"],
        json!({"attempt": 2, "maxAttempts": 3, "gate": ["agent_checks"], "sha": COMMIT_2})
    );

    let events = chat.events(&thread).await;
    assert_contiguous(&events);
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "artifact",
            "agent_status:completed",
            "check_result",
            "rework",
            "agent_status:working",
            "artifact",
            "artifact",
            "agent_status:completed",
            "check_result",
            "thread_state:done",
        ]
    );

    // The rework is a new A2A task in the same context, and says why.
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2, "two attempts, two deliveries");
    assert_eq!(calls[0].context_id, calls[1].context_id, "one context");
    assert_ne!(calls[0].task_id, calls[1].task_id, "a new task");
    assert!(!calls[1].resuming, "the first task had completed");
    assert_eq!(calls[0].text, "verify-red-once fix the login");
    let rework = &calls[1].text;
    assert!(rework.contains("attempt 2"), "{rework}");
    assert!(
        rework.contains("tests::login fails: expected 200, got 500"),
        "{rework}"
    );
    assert!(
        rework.contains("untrusted"),
        "findings are quoted: {rework}"
    );
}

async fn an_agent_that_never_passes_fails_after_the_last_attempt(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(2);

    let frames = read(
        &chat,
        "plain",
        &gated_input(
            &thread,
            "verify-red fix the login",
            json!({"require": ["agent-checks"]}),
        ),
    )
    .await;
    let last = &frames.last().unwrap().event;
    assert_eq!(last["type"], "RUN_ERROR");
    assert_eq!(last["code"], "checks_failed");
    assert!(
        last["message"]
            .as_str()
            .unwrap()
            .contains("after 3 attempts"),
        "{last}"
    );
    assert_eq!(
        last["metadata"]["vymalo.problem"]["title"], "Checks failed",
        "{last}"
    );
    assert_eq!(activities(&frames, "vymalo.check").len(), 3);
    assert_eq!(activities(&frames, "vymalo.rework").len(), 2);
    let final_snapshot = snapshots(&frames).pop().unwrap();
    assert_eq!(final_snapshot.0, "failed");
    assert_eq!(final_snapshot.1["attempt"], 3);

    chat.wait_state(&thread, "failed").await;
    let events = chat.events(&thread).await;
    assert_contiguous(&events);
    assert_eq!(
        shape(&events).last().map(String::as_str),
        Some("thread_state:failed")
    );
    assert_eq!(
        shape(&events)
            .iter()
            .filter(|s| s.as_str() == "rework")
            .count(),
        2
    );
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 3, "three attempts, no more");
    assert!(calls[2].text.contains("attempt 3"), "{}", calls[2].text);
    assert_ne!(calls[1].task_id, calls[2].task_id);
    let record = chat.thread(&thread).await;
    assert_eq!(record["state"], "failed");
    assert_eq!(record["job"]["attempt"], 3);
}

async fn a_thread_may_change_the_attempts_and_gets_the_targets_gate(backend: Backend) {
    let world = World::with(backend, coder_is_gated()).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    // No request: the target's gate applies, with the default attempts.
    let plain_run = Chat::agui_input(&thread_id(3), "run-1", &[("m", "echo hi")], json!({}));
    let frames = read(&chat, "coder", &plain_run).await;
    assert_eq!(
        frames.last().unwrap().event["type"],
        "RUN_ERROR",
        "the `echo` script reports no checks, so the gate refuses it"
    );
    assert_eq!(frames.last().unwrap().event["code"], "checks_failed");
    assert_eq!(world.coder.executions().len(), 3);
    let record = chat.thread(&thread_id(3)).await;
    assert_eq!(record["job"]["gate"], json!(["agent_checks"]));
    assert_eq!(record["job"]["maxAttempts"], 3);

    // A request lowers the attempts.
    let frames = read(
        &chat,
        "coder",
        &gated_input(&thread_id(4), "verify-red x", json!({"maxAttempts": 2})),
    )
    .await;
    assert_eq!(frames.last().unwrap().event["code"], "checks_failed");
    assert_eq!(activities(&frames, "vymalo.check").len(), 2);
    assert_eq!(world.coder.executions().len(), 3 + 2);

    // An agent without a gate has no job in the record and no `job` in its snapshots.
    let ungated = read(
        &chat,
        "plain",
        &Chat::agui_input(&thread_id(5), "run-1", &[("m", "echo hi")], json!({})),
    )
    .await;
    assert_eq!(
        ungated.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );
    assert!(snapshots(&ungated).iter().all(|(_, job)| job.is_null()));
    chat.wait_state(&thread_id(5), "done").await;
    assert!(chat.thread(&thread_id(5)).await.get("job").is_none());
}

async fn a_request_that_weakens_the_gate_or_asks_too_much_is_a_400(backend: Backend) {
    let world = World::with(backend, coder_is_gated()).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let cases = [
        // (agent, gate, what the problem says)
        ("coder", json!({"require": []}), "may add sources"),
        ("coder", json!({"require": []}), "agent-checks"),
        ("coder", json!({"maxAttempts": 11}), "1..=10"),
        ("coder", json!({"maxAttempts": 0}), "1..=10"),
        ("plain", json!({"require": ["ci"]}), "slice 5"),
        (
            "plain",
            json!({"require": ["agent-checks", "verifier"]}),
            "slice 10",
        ),
        ("plain", json!({"verifier": "coder"}), "slice 10"),
        ("plain", json!({"ci": {"required": ["build"]}}), "slice 5"),
        (
            "plain",
            json!({"require": ["nonsense"]}),
            "one of: ci, agent-checks, verifier",
        ),
        ("plain", json!({"surprise": true}), "unknown field"),
        ("plain", json!({"require": "agent-checks"}), "malformed"),
    ];
    for (n, (agent, gate, says)) in cases.into_iter().enumerate() {
        let thread = thread_id(10 + u32::try_from(n).unwrap());
        let resp = chat
            .agui_post(agent, &gated_input(&thread, "echo hi", gate.clone()))
            .await;
        assert_eq!(resp.status().as_u16(), 400, "{gate}");
        assert!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("application/problem+json"),
            "{gate}"
        );
        let problem: Value = resp.json().await.unwrap();
        let detail = problem["detail"].as_str().unwrap_or_default();
        assert!(detail.contains(says), "{gate}: {problem}");
        assert_eq!(
            chat.get(&format!("/api/threads/{thread}")).await.0,
            404,
            "{gate}: nothing was written"
        );
    }
    assert!(world.coder.executions().is_empty());
    assert!(world.plain.executions().is_empty());

    // A request that adds nothing and removes nothing is fine.
    let ok = read(
        &chat,
        "coder",
        &gated_input(
            &thread_id(40),
            "verify-pass all good",
            json!({"require": ["agent-checks"]}),
        ),
    )
    .await;
    assert_eq!(
        ok.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );
    assert_eq!(world.coder.executions().len(), 1);
}

backends!(
    a_failed_check_sends_the_agent_back_and_the_second_attempt_is_green,
    an_agent_that_never_passes_fails_after_the_last_attempt,
    a_thread_may_change_the_attempts_and_gets_the_targets_gate,
    a_request_that_weakens_the_gate_or_asks_too_much_is_a_400,
);
