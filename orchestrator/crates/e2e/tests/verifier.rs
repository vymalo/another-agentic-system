//! The verifier agent in the gate, end to end (ADR 0018, slice 10): the worker pushes, the
//! dispatcher asks the verifier agent over A2A in a context of its own, and its `verdict`
//! artifact decides: findings send the worker back, a pass finishes the job. On the in-memory
//! store and on Postgres, against in-process fake A2A agents over real HTTP.
//!
//! What is checked: one run across the attempts with the verifier as a subagent of its own; the
//! verdict quoted to the worker as untrusted; no verdict, an unusable verdict and a flood of
//! findings are failed checks (capped); a verifier that hangs, breaks or cannot be reached holds
//! the thread (`blocked`, no attempt spent) and is told to stop; a user who writes during a
//! verification abandons it and the next one gets a context of its own; a crash in the middle of
//! a verification gives one verdict, not two.
//!
//! Nothing sleeps to synchronise: each wait is `eventually` on what the store or the agents say.
//! The deadline of the hang test (two seconds) is only how long a passing run takes.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::*;
use jiff::SignedDuration;
use orch_agui_proto::testkit::assert_json_conforms;
use orch_app::GateLayer;
use orch_core::{AgentId, EventBody, EventKind, GatePolicy, Hold, ThreadState};
use orch_ports::{OutboxKind, OutboxPayload};
use orch_testsupport::fake::{VERIFIER_FINDING, VERIFY_SUMMARY, verify_commit};
use orch_testsupport::{Chat, Frame, VerifierScript};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(30);
const COMMIT_1: &str = "0000000000000000000000000000000000000001";
const COMMIT_2: &str = "0000000000000000000000000000000000000002";

fn thread_id(n: u32) -> String {
    format!("00000000-0000-7000-8000-{:012}", 400 + n)
}

/// `plain`'s entry requires the verifier and names `reviewer`, a verifier that answers as
/// `script` says; the deployment waits `timeout` for a verdict.
fn reviewed(script: VerifierScript, timeout: Duration) -> Setup {
    let gate = GateLayer::from_json(&json!({"require": ["verifier"], "verifier": "reviewer"}))
        .unwrap()
        .unwrap();
    Setup {
        reviewer: Some(script),
        target_gates: BTreeMap::from([(AgentId::new("plain"), gate)]),
        gate: GatePolicy {
            verifier_timeout: SignedDuration::try_from(timeout).unwrap(),
            ..GatePolicy::default()
        },
        ..Setup::default()
    }
}

fn input(thread: &str, text: &str, gate: Value) -> Value {
    let extra = if gate.is_null() {
        json!({})
    } else {
        json!({"forwardedProps": {"vymalo.gate": gate}})
    };
    Chat::agui_input(thread, "run-1", &[("msg-1", text)], extra)
}

async fn read(chat: &Chat, body: &Value) -> Vec<Frame> {
    let mut sse = chat.agui_run("plain", body).await;
    let frames = sse.collect_frames(WAIT).await;
    for frame in &frames {
        assert_json_conforms(&frame.event);
    }
    frames
}

fn activities<'a>(frames: &'a [Frame], activity_type: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|f| f.event["activityType"] == activity_type)
        .map(|f| &f.event)
        .collect()
}

/// `(type, name)` of the subagents that start, in order.
fn subagents(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .filter(|f| f.event["type"] == "SUBAGENT_STARTED")
        .map(|f| f.event["name"].as_str().unwrap().to_owned())
        .collect()
}

fn last(frames: &[Frame]) -> &Value {
    &frames.last().unwrap().event
}

async fn findings_then_a_rework_then_a_pass_at_attempt_two(backend: Backend) {
    let world = World::with(
        backend,
        reviewed(VerifierScript::FindingsThenPass, Duration::from_secs(3600)),
    )
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(1);
    let frames = read(
        &chat,
        &input(&thread, "verify-reviewed fix the login", Value::Null),
    )
    .await;

    // One run for both attempts, ended once, in success.
    assert_eq!(last(&frames)["type"], "RUN_FINISHED");
    assert_eq!(last(&frames)["outcome"], json!({"type": "success"}));
    assert_eq!(
        frames
            .iter()
            .filter(|f| f.event["type"] == "RUN_STARTED")
            .count(),
        1
    );
    // The verifier is a subagent of its own, named after it, with an id derived from the
    // verification; every subagent that starts also ends (the checker of the goldens holds it
    // to the same).
    assert_eq!(
        subagents(&frames),
        ["plain", "reviewer", "plain", "reviewer"]
    );
    let ids: Vec<&str> = frames
        .iter()
        .filter(|f| f.event["type"] == "SUBAGENT_STARTED" && f.event["name"] == "reviewer")
        .map(|f| f.event["subagentRunId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["sub-verify-1", "sub-verify-2"]);
    assert_eq!(
        frames
            .iter()
            .filter(|f| f.event["type"] == "SUBAGENT_FINISHED")
            .count(),
        4
    );
    let verdicts: Vec<&Value> = frames
        .iter()
        .filter(|f| {
            f.event["type"] == "SUBAGENT_FINISHED" && f.event["result"].get("passed").is_some()
        })
        .map(|f| &f.event["result"])
        .collect();
    assert_eq!(
        verdicts,
        [&json!({"passed": false}), &json!({"passed": true})]
    );

    // The cards of the verifier source: pending, then the answer, in each attempt.
    let cards: Vec<(String, String)> = activities(&frames, "vymalo.check")
        .iter()
        .map(|c| {
            (
                c["messageId"].as_str().unwrap().to_owned(),
                c["content"]["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        cards,
        [
            ("check-1-1-verifier".to_owned(), "pending".to_owned()),
            ("check-1-1-verifier".to_owned(), "failed".to_owned()),
            ("check-2-2-verifier".to_owned(), "pending".to_owned()),
            ("check-2-2-verifier".to_owned(), "passed".to_owned()),
        ]
    );
    let failed = activities(&frames, "vymalo.check")[1];
    assert_eq!(failed["content"]["source"], "verifier");
    assert_eq!(failed["content"]["findings"], json!([VERIFIER_FINDING]));
    assert_eq!(activities(&frames, "vymalo.rework").len(), 1);

    chat.wait_state(&thread, "done").await;
    let record = chat.thread(&thread).await;
    assert_eq!(
        record["job"],
        json!({"attempt": 2, "maxAttempts": 3, "gate": ["verifier"], "sha": COMMIT_2})
    );

    // The log: the verifier's words are check results, never the worker's updates. The worker
    // finished twice and so did the verification, and only the worker has an actor of its own.
    let events = chat.events(&thread).await;
    assert_contiguous(&events);
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_message",
            "artifact",
            "agent_status:completed",
            "check_result",
            "check_result",
            "rework",
            "agent_status:working",
            "agent_message",
            "artifact",
            "agent_status:completed",
            "check_result",
            "check_result",
            "thread_state:done",
        ]
    );
    assert!(
        events
            .iter()
            .filter(|e| e["kind"] == "agent_status" || e["kind"] == "agent_message")
            .all(|e| e["actor"]["name"] == "plain"),
        "nothing the verifier said became the worker's"
    );
    assert_eq!(
        data_of(&events, "agent_status")
            .iter()
            .filter(|d| d["status"] == "completed")
            .count(),
        2,
        "only the worker's completions complete anything"
    );

    // The verifier was asked once per attempt, in a context of its own.
    let asked = world.reviewer.as_ref().unwrap().executions();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].context_id, format!("{thread}-verify-1-1"));
    assert_eq!(asked[1].context_id, format!("{thread}-verify-2-2"));
    let worked = world.plain.executions();
    assert_eq!(worked.len(), 2);
    assert!(
        asked.iter().all(|a| a.context_id != worked[0].context_id),
        "not the worker's context"
    );
    assert_ne!(asked[0].task_id, worked[0].task_id);
    assert_eq!(
        asked[0].authorization, None,
        "no credentials of the worker's"
    );
    // The prompt says what to check, on which attempt, and quotes what it got from others.
    let prompt = &asked[0].text;
    assert!(prompt.contains(&format!("commit {COMMIT_1}")), "{prompt}");
    assert!(prompt.contains("github.com/acme/demo"), "{prompt}");
    assert!(prompt.contains("attempt 1 of 3"), "{prompt}");
    assert!(prompt.contains("verdict"), "{prompt}");
    assert!(
        prompt.contains("untrusted") && prompt.contains("verify-reviewed fix the login"),
        "the task is quoted as data: {prompt}"
    );
    assert!(
        prompt.contains(VERIFY_SUMMARY),
        "the agent's own account is quoted as data: {prompt}"
    );
    assert!(
        asked[1].text.contains("attempt 2 of 3"),
        "{}",
        asked[1].text
    );
    assert!(asked[1].text.contains(&format!("commit {COMMIT_2}")));
    // The worker is told what the verifier found, as data, and the source is named.
    let rework = &worked[1].text;
    assert!(rework.contains(VERIFIER_FINDING), "{rework}");
    assert!(
        rework.contains("the verifier") && rework.contains("untrusted"),
        "{rework}"
    );
    assert!(!worked[1].resuming, "a rework is a new task");
}

async fn a_verifier_that_never_passes_fails_the_job_after_the_last_attempt(backend: Backend) {
    let world = World::with(
        backend,
        reviewed(VerifierScript::AlwaysFail, Duration::from_secs(3600)),
    )
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(2);
    let frames = read(
        &chat,
        &input(&thread, "verify-reviewed fix the login", Value::Null),
    )
    .await;
    assert_eq!(last(&frames)["type"], "RUN_ERROR");
    assert_eq!(last(&frames)["code"], "checks_failed");
    assert!(
        last(&frames)["message"]
            .as_str()
            .unwrap()
            .contains(VERIFIER_FINDING),
        "{}",
        last(&frames)
    );
    assert_eq!(
        subagents(&frames)
            .iter()
            .filter(|n| *n == "reviewer")
            .count(),
        3
    );
    chat.wait_state(&thread, "failed").await;
    let asked = world.reviewer.as_ref().unwrap().executions();
    assert_eq!(
        asked.len(),
        3,
        "three attempts, three verifications, no more"
    );
    let contexts: Vec<&str> = asked.iter().map(|c| c.context_id.as_str()).collect();
    assert_eq!(
        contexts,
        [
            format!("{thread}-verify-1-1"),
            format!("{thread}-verify-2-2"),
            format!("{thread}-verify-3-3"),
        ]
    );
    assert_eq!(world.plain.executions().len(), 3);
}

/// What a verifier that says nothing usable has said: a failed check, worded, never a pass.
async fn no_verdict_or_an_unusable_one_is_a_failed_check(backend: Backend) {
    let cases = [
        (VerifierScript::NoVerdict, "no verdict", "without reporting"),
        (
            VerifierScript::Garbled,
            "no verdict",
            "`passed` is missing or not a boolean",
        ),
    ];
    for (n, (script, says, detail)) in cases.into_iter().enumerate() {
        let world = World::with(backend, reviewed(script, Duration::from_secs(3600))).await;
        let orch = world.instance("orch-1").await;
        let chat = world.chat(&orch);
        let thread = thread_id(10 + u32::try_from(n).unwrap());
        let frames = read(
            &chat,
            &input(
                &thread,
                "verify-reviewed fix the login",
                json!({"maxAttempts": 1}),
            ),
        )
        .await;
        assert_eq!(last(&frames)["code"], "checks_failed", "{script:?}");
        let cards = activities(&frames, "vymalo.check");
        let answer = cards.last().unwrap();
        assert_eq!(answer["content"]["status"], "failed", "{script:?}");
        let finding = answer["content"]["findings"][0].as_str().unwrap();
        assert!(
            finding.contains(says) && finding.contains(detail),
            "{script:?}: {finding}"
        );
    }
}

/// A verifier is untrusted: sixty findings of two kilobytes each are kept as at most twenty
/// items and sixteen kilobytes, in the log as in the card.
async fn a_flood_of_findings_is_capped(backend: Backend) {
    let world = World::with(
        backend,
        reviewed(VerifierScript::Flood, Duration::from_secs(3600)),
    )
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = thread_id(20);
    let frames = read(
        &chat,
        &input(
            &thread,
            "verify-reviewed fix the login",
            json!({"maxAttempts": 1}),
        ),
    )
    .await;
    let cards = activities(&frames, "vymalo.check");
    let findings = cards.last().unwrap()["content"]["findings"]
        .as_array()
        .unwrap()
        .clone();
    let bytes: usize = findings.iter().map(|f| f.as_str().unwrap().len()).sum();
    assert!(findings.len() <= 20, "{} findings", findings.len());
    assert!(bytes <= 16 * 1024, "{bytes} bytes");
    assert!(
        findings
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("omitted")
            || findings.last().unwrap().as_str().unwrap().contains("[cut]"),
        "the cut is said"
    );
}

/// The verifier does not answer in time: the thread waits for the user (no attempt is spent),
/// the verifier is told to stop and its row is dropped. The deadline is the core's own timer,
/// armed in the commit that asked, and fired by the inbox worker.
async fn a_verifier_that_does_not_answer_in_time_holds_the_thread(backend: Backend) {
    let world = World::with(
        backend,
        reviewed(VerifierScript::Hang, Duration::from_secs(2)),
    )
    .await;
    let orch = world.instance("orch-1").await;
    let node = world.node("inbox-1").await;
    let inbox = node.spawn_inbox(fast_inbox(), "inbox-1");
    let chat = world.chat(&orch);
    let thread = thread_id(30);
    let frames = read(
        &chat,
        &input(&thread, "verify-reviewed fix the login", Value::Null),
    )
    .await;

    // The run ends in an interrupt the user can answer, saying why; it is not a failure.
    let end = last(&frames);
    assert_eq!(end["type"], "RUN_FINISHED", "{end}");
    assert_eq!(end["outcome"]["type"], "interrupt", "{end}");
    assert!(
        end["outcome"]["interrupts"][0]["message"]
            .as_str()
            .unwrap()
            .contains("did not answer in time"),
        "{end}"
    );
    assert_eq!(subagents(&frames), ["plain", "reviewer"]);
    // Only the verifier can have held this thread (no CI is required), so its subagent ends in
    // an error, not as if it had been cancelled.
    assert!(
        frames.iter().any(|f| f.event["type"] == "SUBAGENT_ERROR"
            && f.event["subagentRunId"] == "sub-verify-1"
            && f.event["code"] == "verifier_failed"
            && f.event["message"]
                .as_str()
                .is_some_and(|m| m.contains("did not answer in time"))),
        "the verifier's subagent ends when the wait does, in an error"
    );

    chat.wait_state(&thread, "blocked").await;
    let id = thread.parse().unwrap();
    let held = node.thread(id).await;
    assert_eq!(held.state, ThreadState::Blocked);
    assert_eq!(held.job.hold, Some(Hold::VerifierTimeout));
    assert_eq!(held.job.attempt, 1, "a timeout spends no attempt");
    let events = node.events(id).await;
    assert!(events.iter().any(|e| matches!(
        &e.body,
        EventBody::Error(d) if d.retryable && d.message.contains("did not answer in time")
    )));

    // The verifier is told to stop, and the row that asked it is over.
    let reviewer = world.reviewer.as_ref().unwrap();
    eventually("the verifier is told to stop", || async {
        (reviewer.cancels().len() == 1).then_some(())
    })
    .await;
    eventually("the verification row is dropped", || async {
        node.open_outbox(id)
            .await
            .iter()
            .all(|row| row.kind != OutboxKind::Verify)
            .then_some(())
    })
    .await;
    assert_eq!(reviewer.executions().len(), 1, "asked once");
    assert_eq!(
        world.plain.executions().len(),
        1,
        "the worker was not sent back"
    );
    inbox.shutdown().await;
}

/// The verifier cannot do its job (its task fails, or it cannot be reached): the thread waits
/// for the user and no attempt is spent.
async fn a_verifier_that_breaks_or_cannot_be_reached_holds_the_thread(backend: Backend) {
    for (n, (script, stop, says)) in [
        (
            VerifierScript::Broken,
            false,
            "the verifier's task ended without an answer",
        ),
        (
            VerifierScript::AlwaysPass,
            true,
            "the verifier could not be used: the agent could not be reached",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let world = World::with(backend, reviewed(script, Duration::from_secs(3600))).await;
        if stop {
            world.reviewer.as_ref().unwrap().stop();
        }
        let orch = world.instance("orch-1").await;
        let node = world.node("inbox-1").await;
        let chat = world.chat(&orch);
        let thread = thread_id(40 + u32::try_from(n).unwrap());
        let frames = read(
            &chat,
            &input(&thread, "verify-reviewed fix the login", Value::Null),
        )
        .await;
        let end = last(&frames);
        assert_eq!(end["outcome"]["type"], "interrupt", "{script:?}: {end}");
        assert!(
            end["outcome"]["interrupts"][0]["message"]
                .as_str()
                .unwrap()
                .contains(says),
            "{script:?}: {end}"
        );
        let held = node.thread(thread.parse().unwrap()).await;
        assert_eq!(held.state, ThreadState::Blocked, "{script:?}");
        assert_eq!(held.job.hold, Some(Hold::VerifierFailed), "{script:?}");
        assert_eq!(held.job.attempt, 1, "{script:?}: no attempt is spent");
        assert_eq!(world.plain.executions().len(), 1, "{script:?}");
    }
}

/// A user who writes while the work is verified abandons that verification: the verifier is
/// told to stop, the worker is sent the message without an attempt being spent, and the next
/// verification, in the same attempt, asks in a context of its own.
async fn a_message_during_a_verification_abandons_it(backend: Backend) {
    let world = World::with(
        backend,
        reviewed(VerifierScript::Hang, Duration::from_secs(3600)),
    )
    .await;
    let orch = world.instance("orch-1").await;
    let node = world.node("inbox-1").await;
    let chat = world.chat(&orch);
    let thread = chat
        .create_thread("plain", "verify-reviewed fix the login", None)
        .await;
    let id = thread.parse().unwrap();
    let reviewer = world.reviewer.as_ref().unwrap();
    eventually("the verifier is asked", || async {
        (reviewer.executions().len() == 1).then_some(())
    })
    .await;
    assert_eq!(node.thread(id).await.state, ThreadState::Verifying);

    // The AG-UI surface keeps a run open across a verification and refuses a second one, so the
    // message comes the way a producer that is not AG-UI would send it (the application).
    let event = chat
        .seed_message(&thread, "verify-reviewed and the logout too")
        .await;
    assert_eq!(event["kind"], "user_message");
    eventually("the first verification is told to stop", || async {
        (reviewer.cancels().len() == 1).then_some(())
    })
    .await;
    eventually("the work is verified again", || async {
        (reviewer.executions().len() == 2).then_some(())
    })
    .await;
    let asked = reviewer.executions();
    assert_eq!(asked[0].context_id, format!("{thread}-verify-1-1"));
    assert_eq!(
        asked[1].context_id,
        format!("{thread}-verify-1-2"),
        "the same attempt, a context of its own"
    );
    let now = node.thread(id).await;
    assert_eq!(
        (now.state, now.job.attempt, now.job.verification),
        (ThreadState::Verifying, 1, 2)
    );
    assert_eq!(
        world.plain.executions().len(),
        2,
        "the message reached the worker"
    );

    // Cancelling ends the second verification the same way.
    assert_eq!(chat.cancel(&thread).await, 202);
    chat.wait_state(&thread, "cancelled").await;
    eventually("the second verification is told to stop", || async {
        (reviewer.cancels().len() == 2).then_some(())
    })
    .await;
    let events = node.events(id).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind() == EventKind::CheckResult)
            .count(),
        2,
        "two pending cards and no verdict: {:?}",
        shape(
            &events
                .iter()
                .map(|e| serde_json::to_value(e).unwrap())
                .collect::<Vec<_>>()
        )
    );
}

/// The process that asked the verifier dies while it works. Another takes the row over when the
/// lease lapses and re-attaches to the verifier's task: the verifier is asked once, the verdict
/// is one event, and the job is done.
async fn a_crash_in_the_middle_of_a_verification_gives_one_verdict(backend: Backend) {
    let world = World::with(
        backend,
        reviewed(VerifierScript::GatedPass, Duration::from_secs(3600)),
    )
    .await;
    let first = world.instance("orch-1").await;
    let node = world.node("inbox-1").await;
    let chat = world.chat(&first);
    let thread = chat
        .create_thread("plain", "verify-reviewed fix the login", None)
        .await;
    let id = thread.parse().unwrap();
    let reviewer = world.reviewer.as_ref().unwrap();
    // The request reached the verifier and was recorded on the row (its task is the row's).
    eventually("the verification is under way", || async {
        node.open_outbox(id)
            .await
            .into_iter()
            .find(|row| row.kind == OutboxKind::Verify && row.task_id.is_some())
            .map(|_| ())
    })
    .await;
    let rows = node.open_outbox(id).await;
    let row = rows.iter().find(|r| r.kind == OutboxKind::Verify).unwrap();
    assert!(matches!(
        row.payload,
        OutboxPayload::Verify {
            attempt: 1,
            verification: 1,
            ..
        }
    ));
    assert_eq!(
        row.task_id.as_deref(),
        Some(reviewer.executions()[0].task_id.as_str())
    );

    // The process dies; the verifier finishes its task in the meantime.
    first.kill();
    reviewer.release_gate();
    let second = world.instance("orch-2").await;
    let chat = world.chat(&second);
    chat.wait_state(&thread, "done").await;

    let events = node.events(id).await;
    let checks: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::CheckResult(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(checks.len(), 2, "pending, then the one verdict: {checks:?}");
    assert!(
        checks.iter().all(|c| !c.stale),
        "no late duplicate was recorded"
    );
    assert_eq!(
        checks
            .iter()
            .filter(|c| c.status == orch_core::CheckStatus::Passed)
            .count(),
        1
    );
    assert_eq!(
        reviewer.executions().len(),
        1,
        "the verifier was asked once"
    );
    assert_eq!(
        reviewer.rpc_count("send_streaming_message"),
        1,
        "and not sent the request again"
    );
    assert_eq!(world.plain.executions().len(), 1);
    assert_eq!(
        chat.thread(&thread).await["job"]["sha"],
        verify_commit(1),
        "the job is done on the commit that was verified"
    );
}

backends!(
    findings_then_a_rework_then_a_pass_at_attempt_two,
    a_verifier_that_never_passes_fails_the_job_after_the_last_attempt,
    no_verdict_or_an_unusable_one_is_a_failed_check,
    a_flood_of_findings_is_capped,
    a_verifier_that_does_not_answer_in_time_holds_the_thread,
    a_verifier_that_breaks_or_cannot_be_reached_holds_the_thread,
    a_message_during_a_verification_abandons_it,
    a_crash_in_the_middle_of_a_verification_gives_one_verdict,
);
