//! The verification gate in the projection (ADR 0018, `docs/api/agui.md`): a run stays open
//! while the thread is verified, a finished agent is a finished subagent and not a finished run,
//! a check is a `vymalo.check` card, a rework is a `vymalo.rework` card and the next attempt's
//! subagent, and running out of attempts is `RUN_ERROR` with `checks_failed`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_agui_proto::testkit::assert_conforms;
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, AgentTaskState, CheckResult, CheckSource,
    CheckStatus, ErrorData, Event, EventBody, GatePolicy, ThreadState, ThreadStateData, Timestamp,
    UserId, UserMessageData,
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
    assert!(story[finished + 2].contains("check-1-1-agent_checks vymalo.check"));
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
        .find(|f| f.event.event_type().as_str() == "ACTIVITY_SNAPSHOT" && line_has(f, "check-1-1-"))
        .unwrap();
    let check_1 = serde_json::to_value(&check_1.event).unwrap();
    assert_eq!(check_1["activityType"], "vymalo.check");
    assert_eq!(check_1["replace"], true);
    assert_eq!(check_1["content"]["source"], "agent_checks");
    assert_eq!(check_1["content"]["status"], "failed");
    assert_eq!(check_1["content"]["attempt"], 1);
    assert_eq!(check_1["content"]["findings"], json!(["it fails"]));
    assert!(
        text.contains("check-2-2-agent_checks vymalo.check"),
        "{text}"
    );

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

// ---- review fixes -----------------------------------------------------------------------

fn event(seq: i64, actor: Actor, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: support::log::thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    }
}

fn agent_status(seq: i64, status: AgentStatus) -> Event {
    event(
        seq,
        Actor::agent(&AgentId::new("plain"), None),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: None,
        }),
    )
}

fn user_message(seq: i64) -> Event {
    event(
        seq,
        Actor::user(&UserId::new("alice@example.com")),
        EventBody::UserMessage(UserMessageData::new("go")),
    )
}

fn check(seq: i64, source: CheckSource, status: CheckStatus, stale: bool) -> Event {
    event(
        seq,
        Actor::system(),
        EventBody::CheckResult(CheckResult {
            source,
            name: None,
            attempt: 1,
            commit: None,
            status,
            summary: None,
            stale,
            findings: vec![],
        }),
    )
}

fn fold(events: &[Event], gate: GatePolicy) -> (Vec<Frame>, Projector) {
    let mut projector = Projector::new(meta_under(gate));
    let frames = events
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    (frames, projector)
}

/// Every sha any snapshot of the frames carries.
fn shas(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .filter_map(snapshot_job)
        .filter_map(|job| job["sha"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn the_sha_of_the_snapshot_is_the_pushed_commit_not_the_commit_the_checks_ran_on() {
    let abc = format!("{:040x}", 1);
    let def = format!("{:040x}", 2);
    // The agent pushed `def` and its checks ran on `abc`.
    let (_, frames, _) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 2 },
            Action::Checks {
                passed: true,
                commit: 1,
            },
            COMPLETED,
        ],
        &gate(),
    );
    let all = flat(&frames);
    let found = shas(&all);
    assert!(!found.is_empty());
    assert!(found.iter().all(|s| *s == def), "{found:?}");
    assert!(!found.contains(&abc));
    // ... and the check card itself still says which commit it ran on.
    let card = all
        .iter()
        .find(|f| line_has(f, "vymalo.check"))
        .map(|f| serde_json::to_value(&f.event).unwrap())
        .unwrap();
    assert_eq!(card["content"]["commit"], abc);
}

#[test]
fn checks_without_a_pushed_branch_give_the_snapshots_no_sha() {
    let (_, frames, _) = project(
        &[
            user(),
            WORKING,
            Action::Checks {
                passed: true,
                commit: 1,
            },
            COMPLETED,
        ],
        &gate(),
    );
    let all = flat(&frames);
    assert!(all.iter().filter_map(snapshot_job).count() >= 3);
    assert_eq!(shas(&all), Vec::<String>::new());
}

#[test]
fn a_second_verification_of_the_same_attempt_is_a_card_of_its_own() {
    // CI (which this build does not honour yet, so the events are made by hand) has not answered
    // when the user writes: the verification is abandoned without using the attempt, and the
    // agent finishes again. The two verifications of attempt 1 must not share a card.
    let gate = GatePolicy::requiring([CheckSource::Ci]);
    let events = [
        user_message(1),
        agent_status(2, AgentStatus::Working),
        agent_status(3, AgentStatus::Completed),
        check(4, CheckSource::Ci, CheckStatus::Pending, false),
        user_message(5),
        agent_status(6, AgentStatus::Working),
        agent_status(7, AgentStatus::Completed),
        check(8, CheckSource::Ci, CheckStatus::Pending, false),
        check(9, CheckSource::Ci, CheckStatus::Passed, false),
    ];
    let (frames, _) = fold(&events, gate);
    all_conform(&frames);
    let ids: Vec<String> = frames
        .iter()
        .filter(|f| line_has(f, "vymalo.check"))
        .map(|f| serde_json::to_value(&f.event).unwrap())
        .map(|e| e["messageId"].as_str().unwrap().to_owned())
        .collect();
    // Within one verification the pending card and its answer share an id (`replace`).
    assert_eq!(ids, ["check-1-1-ci", "check-1-2-ci", "check-1-2-ci"]);
}

#[test]
fn a_stale_check_card_never_takes_a_check_id() {
    let gate = GatePolicy::requiring([CheckSource::Ci]);
    let events = [
        user_message(1),
        agent_status(2, AgentStatus::Working),
        agent_status(3, AgentStatus::Completed),
        check(4, CheckSource::Ci, CheckStatus::Pending, false),
        check(5, CheckSource::Ci, CheckStatus::Passed, true),
        check(6, CheckSource::Verifier, CheckStatus::Failed, true),
    ];
    let (frames, projector) = fold(&events, gate);
    let ids: Vec<String> = frames
        .iter()
        .filter(|f| line_has(f, "vymalo.check"))
        .map(|f| serde_json::to_value(&f.event).unwrap())
        .map(|e| e["messageId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["check-1-1-ci", "evt-5", "evt-6"]);
    assert_eq!(
        projector.thread_state(),
        ThreadState::Verifying,
        "and they decided nothing"
    );
}

#[test]
fn a_hold_ends_the_run_in_an_interrupt_the_user_can_answer() {
    // CI did not report in time: `error{retryable}` then `blocked`, in a thread that was being
    // verified. It projects as a blocked thread does, not as a failed delivery.
    let gate = GatePolicy::requiring([CheckSource::Ci]);
    let mut events = vec![
        user_message(1),
        agent_status(2, AgentStatus::Working),
        agent_status(3, AgentStatus::Completed),
        check(4, CheckSource::Ci, CheckStatus::Pending, false),
        event(
            5,
            Actor::system(),
            EventBody::Error(ErrorData {
                message: "CI did not report in time".to_owned(),
                retryable: true,
            }),
        ),
        event(
            6,
            Actor::system(),
            EventBody::ThreadState(ThreadStateData {
                state: ThreadState::Blocked,
            }),
        ),
    ];
    let (frames, projector) = fold(&events, gate.clone());
    all_conform(&frames);
    let last = frames.last().unwrap();
    let orch_agui_proto::Event::RunFinished(finished) = &last.event else {
        panic!("{:?}", lines(&frames));
    };
    let value = serde_json::to_value(finished).unwrap();
    assert_eq!(value["outcome"]["type"], "interrupt", "{value}");
    let asked = &value["outcome"]["interrupts"][0];
    assert_eq!(asked["id"], "int-6");
    assert_eq!(asked["reason"], "input_required");
    assert_eq!(asked["message"], "CI did not report in time");
    assert!(!lines(&frames).iter().any(|l| l.starts_with("RUN_ERROR")));
    // The interrupt is one the thread holds, so an answer (`resume`) can name it.
    let view = projector.view(&UserId::new("alice@example.com"));
    let orch_agui_projection::ThreadView::Known(known) = view else {
        panic!("the thread is known");
    };
    assert_eq!(known.open_interrupts, ["int-6"]);
    assert_eq!(known.state, ThreadState::Blocked);

    // A delivery failure of the agent, out of any verification, is still a delivery failure.
    events.truncate(2);
    events.push(event(
        3,
        Actor::system(),
        EventBody::Error(ErrorData {
            message: "cannot deliver".to_owned(),
            retryable: true,
        }),
    ));
    events.push(event(
        4,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData {
            state: ThreadState::Blocked,
        }),
    ));
    let (frames, _) = fold(&events, gate);
    assert!(
        lines(&frames)
            .last()
            .unwrap()
            .starts_with("RUN_ERROR delivery_failed")
    );
}
