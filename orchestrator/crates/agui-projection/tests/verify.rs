//! The verification gate in the projection (ADR 0018, `docs/api/agui.md`): a run stays open
//! while the thread is verified, a finished agent is a finished subagent and not a finished run,
//! a check is a `vymalo.check` card, a rework is a `vymalo.rework` card and the next attempt's
//! subagent, and running out of attempts is `RUN_ERROR` with `checks_failed`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_agui_proto::testkit::assert_conforms;
use orch_core::{
    AgentTaskState, CheckResult, CheckSource, CheckStatus, Event, EventBody, GatePolicy,
    ThreadState,
};
use serde_json::{Value, json};
use support::log::{Action, build_under, gate, meta_under};
use support::{lines, verify};

const WORKING: Action = Action::Status(AgentTaskState::Working, None);
const COMPLETED: Action = Action::Status(AgentTaskState::Completed, None);

fn user() -> Action {
    Action::User {
        text: "fix the login".to_owned(),
        ids: true,
    }
}

fn project(actions: &[Action], gate: &GatePolicy) -> (Vec<Event>, Vec<Vec<Frame>>, Projector) {
    let events = build_under(actions, gate);
    let mut projector = Projector::new(meta_under(gate.clone()));
    let frames: Vec<Vec<Frame>> = events
        .iter()
        .map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    (events, frames, projector)
}

fn flat(frames: &[Vec<Frame>]) -> Vec<Frame> {
    frames.iter().flatten().cloned().collect()
}

/// The lines of the frames that are not about text or plain status.
fn story(frames: &[Frame]) -> Vec<String> {
    lines(frames)
        .into_iter()
        .filter(|l| {
            !l.starts_with("TEXT_MESSAGE")
                && !l.contains("vymalo.status")
                && !l.contains("vymalo.artifact")
        })
        .collect()
}

fn snapshot_job(frame: &Frame) -> Option<Value> {
    match &frame.event {
        orch_agui_proto::Event::StateSnapshot(e) => e.snapshot.get("job").cloned(),
        _ => None,
    }
}

fn all_conform(frames: &[Frame]) {
    for frame in frames {
        assert_conforms(&frame.event);
    }
    verify::check(frames).unwrap();
}

#[test]
fn a_finished_agent_under_a_gate_ends_its_subagent_and_keeps_the_run_open() {
    // The agent passes its own checks: verifying is over in the same transaction, so the run
    // ends, but only after the check card, and the snapshot before it says `verifying`.
    let (_, frames, projector) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            Action::Checks {
                passed: true,
                commit: 1,
            },
            COMPLETED,
        ],
        &gate(),
    );
    let all = flat(&frames);
    all_conform(&all);
    let story = story(&all);
    let finished = story
        .iter()
        .position(|l| l.starts_with("SUBAGENT_FINISHED"))
        .unwrap();
    assert!(story[finished + 1].starts_with("STATE_SNAPSHOT verifying"));
    assert!(story[finished + 2].contains("check-1-agent_checks vymalo.check"));
    assert!(story[finished + 3].starts_with("STATE_SNAPSHOT done"));
    assert!(story[finished + 4].starts_with("RUN_FINISHED r-1 success"));
    assert!(!projector.run_open());
    // Exactly one run, and no terminal event before the end.
    let terminals = story
        .iter()
        .filter(|l| l.starts_with("RUN_FINISHED") || l.starts_with("RUN_ERROR"))
        .count();
    assert_eq!(terminals, 1);

    // The job in the snapshots: the attempt, the attempts there are, the gate and the commit.
    let jobs: Vec<Value> = all.iter().filter_map(snapshot_job).collect();
    assert!(jobs.iter().all(|j| j["gate"] == json!(["agent_checks"])));
    assert!(jobs.iter().all(|j| j["maxAttempts"] == 3));
    assert_eq!(
        jobs.last().unwrap(),
        &json!({"attempt": 1, "maxAttempts": 3, "gate": ["agent_checks"],
                "sha": format!("{:040x}", 1)})
    );
}

#[test]
fn a_run_stays_open_while_the_thread_is_verified() {
    // The log so far: the user, the agent working, the agent completed. Nothing has answered
    // yet (with the gate of this build the answer follows in the same transaction; a slower
    // source, CI or a verifier, leaves the log here for a while).
    let events = build_under(&[user(), WORKING, COMPLETED], &gate());
    let mut projector = Projector::new(meta_under(gate()));
    for e in &events[..3] {
        projector.apply(e, Audience::Viewer);
    }
    assert_eq!(projector.thread_state(), ThreadState::Verifying);
    assert!(projector.run_open(), "verifying is an active state");
    // A client that joins now is told: the run, and where the job stands.
    let preamble = projector.resume_preamble();
    let types: Vec<&str> = preamble
        .iter()
        .map(|f| f.event.event_type().as_str())
        .collect();
    assert_eq!(types, ["RUN_STARTED", "STATE_SNAPSHOT"]);
    assert!(
        lines(&preamble)[1].starts_with("STATE_SNAPSHOT verifying"),
        "the agent has finished, so no subagent is open"
    );
    assert_eq!(snapshot_job(&preamble[1]).unwrap()["attempt"], 1);
    assert!(preamble.iter().all(|f| f.resume_id.is_none()));
    all_conform(&preamble);
}

#[test]
fn a_failed_check_is_a_card_then_a_rework_and_the_next_attempts_subagent() {
    let (_, frames, projector) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            Action::Checks {
                passed: false,
                commit: 1,
            },
            COMPLETED,
            WORKING,
            Action::Branch { commit: 2 },
            Action::Checks {
                passed: true,
                commit: 2,
            },
            COMPLETED,
        ],
        &gate(),
    );
    let all = flat(&frames);
    all_conform(&all);
    let story = story(&all);
    let text = story.join("\n");

    // Attempt 1's card is failed, with the findings; attempt 2's is passed. Their ids come from
    // the attempt and the source.
    let check_1 = all
        .iter()
        .find(|f| f.event.event_type().as_str() == "ACTIVITY_SNAPSHOT" && line_has(f, "check-1-"))
        .unwrap();
    let check_1 = serde_json::to_value(&check_1.event).unwrap();
    assert_eq!(check_1["activityType"], "vymalo.check");
    assert_eq!(check_1["replace"], true);
    assert_eq!(check_1["content"]["source"], "agent_checks");
    assert_eq!(check_1["content"]["status"], "failed");
    assert_eq!(check_1["content"]["attempt"], 1);
    assert_eq!(check_1["content"]["findings"], json!(["it fails"]));
    assert!(text.contains("check-2-agent_checks vymalo.check"), "{text}");

    // The rework card, then the subagent of attempt 2, then the state that says so.
    let rework = story
        .iter()
        .position(|l| l.contains("rework-2 vymalo.rework"))
        .unwrap();
    assert!(story[rework].contains("\"attempt\":2"), "{}", story[rework]);
    assert!(story[rework].contains("\"maxAttempts\":3"));
    assert!(
        story[rework + 1].starts_with("SUBAGENT_STARTED sub-"),
        "{}",
        story[rework + 1]
    );
    assert!(
        story[rework + 1].ends_with(" plain"),
        "the agent, by its id"
    );
    assert!(story[rework + 2].starts_with("STATE_SNAPSHOT queued"));
    let job = all
        .iter()
        .filter_map(snapshot_job)
        .find(|j| j["attempt"] == 2)
        .unwrap();
    assert!(
        job.get("sha").is_none(),
        "the next attempt pushes its own commit"
    );

    // One run through all of it, and the agent finishes once per attempt.
    assert_eq!(text.matches("RUN_STARTED").count(), 1);
    assert_eq!(text.matches("SUBAGENT_STARTED").count(), 2);
    assert_eq!(text.matches("SUBAGENT_FINISHED").count(), 2);
    assert!(
        story
            .last()
            .unwrap()
            .starts_with("RUN_FINISHED r-1 success")
    );
    assert!(!projector.run_open());
}

fn line_has(frame: &Frame, needle: &str) -> bool {
    lines(std::slice::from_ref(frame))[0].contains(needle)
}

#[test]
fn running_out_of_attempts_is_run_error_checks_failed() {
    let one = GatePolicy {
        max_attempts: 1,
        ..gate()
    };
    let (_, frames, projector) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            Action::Checks {
                passed: false,
                commit: 1,
            },
            COMPLETED,
        ],
        &one,
    );
    let all = flat(&frames);
    all_conform(&all);
    let last = all.last().unwrap();
    let orch_agui_proto::Event::RunError(error) = &last.event else {
        panic!("{:?}", lines(&all));
    };
    assert_eq!(error.code.as_deref(), Some("checks_failed"));
    assert!(
        error.message.contains("after 1 attempts"),
        "{}",
        error.message
    );
    let problem = &error.base.metadata.as_ref().unwrap()["vymalo.problem"];
    assert_eq!(problem["title"], "Checks failed");
    assert!(!projector.run_open());
    // The state before it says failed, with the job where it stopped.
    let snapshot = all
        .iter()
        .rev()
        .find(|f| snapshot_job(f).is_some())
        .unwrap();
    assert!(lines(std::slice::from_ref(snapshot))[0].starts_with("STATE_SNAPSHOT failed"));
    assert_eq!(snapshot_job(snapshot).unwrap()["attempt"], 1);
    // No rework was announced, and the agent's subagent was not marked failed: it did its work.
    let text = story(&all).join("\n");
    assert!(!text.contains("vymalo.rework"), "{text}");
    assert!(!text.contains("SUBAGENT_ERROR"), "{text}");
}

#[test]
fn a_delivery_failure_after_a_rework_is_still_a_delivery_failure() {
    // The rework's delegation is dead-lettered: the invocation the projection opened for the
    // next attempt ends with the delivery error, and the run with it.
    let (_, frames, projector) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            Action::Checks {
                passed: false,
                commit: 1,
            },
            COMPLETED,
            Action::Delivery { retryable: true },
        ],
        &gate(),
    );
    let all = flat(&frames);
    all_conform(&all);
    let last = lines(&all).pop().unwrap();
    assert!(last.starts_with("RUN_ERROR delivery_failed"), "{last}");
    assert!(story(&all).iter().any(|l| l.starts_with("SUBAGENT_ERROR")));
    assert!(!projector.run_open());
}

#[test]
fn without_a_gate_nothing_changes_and_no_snapshot_has_a_job() {
    let (_, frames, projector) = project(
        &[user(), WORKING, Action::Artifact, COMPLETED],
        &GatePolicy::default(),
    );
    let all = flat(&frames);
    all_conform(&all);
    assert!(all.iter().all(|f| snapshot_job(f).is_none()));
    let text = lines(&all).join("\n");
    assert!(!text.contains("verifying"), "{text}");
    assert!(text.contains("RUN_FINISHED r-1 success"));
    assert!(!projector.run_open());
}

#[test]
fn a_stale_answer_is_a_card_of_its_own_and_changes_nothing_else() {
    let events = build_under(&[user(), WORKING, COMPLETED], &gate());
    let mut projector = Projector::new(meta_under(gate()));
    for e in &events[..3] {
        projector.apply(e, Audience::Viewer);
    }
    assert_eq!(projector.thread_state(), ThreadState::Verifying);
    let stale = Event {
        seq: 4,
        body: EventBody::CheckResult(CheckResult {
            source: CheckSource::AgentChecks,
            name: None,
            attempt: 1,
            commit: Some("f".repeat(40)),
            status: CheckStatus::Passed,
            summary: None,
            stale: true,
            findings: vec![],
        }),
        ..events[2].clone()
    };
    let frames = projector.apply(&stale, Audience::Viewer);
    assert_eq!(frames.len(), 1, "{:?}", lines(&frames));
    assert!(
        line_has(&frames[0], "evt-4 vymalo.check"),
        "{:?}",
        lines(&frames)
    );
    assert_eq!(projector.thread_state(), ThreadState::Verifying);
    assert!(projector.run_open());
}
