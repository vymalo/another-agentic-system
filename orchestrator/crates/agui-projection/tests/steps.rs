//! Nested steps (ADR 0025, `docs/api/agui.md`, "Nested steps"), row by row, on hand-written logs:
//! a sub-agent step is a subagent of the run nested under the one it runs in, every step is a
//! `vymalo.step` activity attributed to its enclosing subagent, and what is open closes with the
//! invocation that holds it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentMessageData, AgentStatus, AgentStatusData, AgentStepData, Event,
    EventBody, StepKind, StepPhase, StepState, ThreadState, ThreadStateData, Timestamp, UserId,
    UserMessageData,
};
use serde_json::{Value, json};
use support::log::{meta, thread_id};
use support::{lines, verify};

fn ev(seq: i64, actor: Actor, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    }
}

fn plain() -> Actor {
    Actor::agent(&AgentId::new("plain"), None)
}

fn user(seq: i64, text: &str) -> Event {
    ev(
        seq,
        Actor::user(&UserId::new("alice@example.com")),
        EventBody::UserMessage(UserMessageData::new(text)),
    )
}

fn status(seq: i64, status: AgentStatus, detail: Option<&str>) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: detail.map(str::to_owned),
        }),
    )
}

fn thread(seq: i64, state: ThreadState) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData { state }),
    )
}

/// An `agent_step` event. `path` is the chain of ids the step runs under.
fn step(
    seq: i64,
    id: &str,
    path: &[&str],
    kind: StepKind,
    state: StepState,
    phase: StepPhase,
) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStep(AgentStepData {
            id: id.to_owned(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
            kind,
            label: format!("label {id}"),
            state,
            phase,
            icon: None,
            detail: None,
            input: None,
            output: None,
            io_dropped: false,
        }),
    )
}

fn with(mut event: Event, icon: Option<&str>, detail: Option<&str>) -> Event {
    if let EventBody::AgentStep(d) = &mut event.body {
        d.icon = icon.map(str::to_owned);
        d.detail = detail.map(str::to_owned);
    }
    event
}

use StepKind::{Command, Subagent, Tool};
use StepPhase::{End, Start, Update};
use StepState::{Canceled, Completed, Failed, Running, Waiting};

/// Projects `events`, checking the stream, one entry per event.
fn project(events: &[Event]) -> Vec<Vec<Frame>> {
    let mut projector = Projector::new(meta());
    let frames: Vec<Vec<Frame>> = events
        .iter()
        .map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    verify::check(&support::flatten(&frames)).unwrap();
    frames
}

fn all_lines(events: &[Event]) -> Vec<String> {
    lines(&support::flatten(&project(events)))
}

/// The lines after the first one that starts with `marker`, the marker's own included.
fn from(got: &[String], marker: &str) -> Vec<String> {
    got.iter()
        .skip_while(|l| !l.starts_with(marker))
        .cloned()
        .collect()
}

/// The activity with message id `id`, as its last snapshot said it.
fn activity(frames: &[Frame], id: &str) -> Value {
    frames
        .iter()
        .rev()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) if e.message_id.as_str() == id => {
                Some(serde_json::to_value(e).unwrap())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no activity {id}"))
}

/// A sub-agent step `OpenCode` (seq 3) with a command under it (seq 4).
fn nested() -> Vec<Event> {
    vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        step(3, "t/o", &[], Subagent, Running, Start),
        step(4, "t/c", &["t/o"], Command, Running, Start),
    ]
}

#[test]
fn a_subagent_step_is_a_subagent_of_its_enclosing_one_and_its_activity_is_the_enclosings() {
    let got = all_lines(&nested());
    assert_eq!(
        from(&got, "SUBAGENT_STARTED sub-step-3"),
        [
            "SUBAGENT_STARTED sub-step-3 label t/o in sub-2",
            "ACTIVITY_SNAPSHOT step-3 vymalo.step {\"id\":\"t/o\",\"kind\":\"subagent\",\"label\":\"label t/o\",\"path\":[],\"startedAt\":\"2027-01-15T08:00:03Z\",\"state\":\"running\"} @sub-2  id:3",
            // the command is a step of the sub-agent: attributed to it, not to the agent
            "ACTIVITY_SNAPSHOT step-4 vymalo.step {\"id\":\"t/c\",\"kind\":\"command\",\"label\":\"label t/c\",\"path\":[\"t/o\"],\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"running\"} @sub-step-3  id:4",
        ]
    );
}

#[test]
fn the_first_step_of_a_thread_opens_the_run_and_the_agents_invocation() {
    // a log whose first event is a step: the run, the invocation and the state come with it
    let got = all_lines(&[step(1, "t/a", &[], Tool, Running, Start)]);
    assert_eq!(
        got,
        [
            "RUN_STARTED run-1",
            "STATE_SNAPSHOT working",
            "SUBAGENT_STARTED sub-1 plain",
            "ACTIVITY_SNAPSHOT step-1 vymalo.step {\"id\":\"t/a\",\"kind\":\"tool\",\"label\":\"label t/a\",\"path\":[],\"startedAt\":\"2027-01-15T08:00:01Z\",\"state\":\"running\"} @sub-1  id:1",
        ]
    );
}

#[test]
fn a_step_that_moves_a_queued_thread_to_working_says_so() {
    // the run is open and queued (the user wrote), and no status said `working`
    let got = all_lines(&[user(1, "go"), step(2, "t/a", &[], Tool, Running, Start)]);
    assert_eq!(
        from(&got, "SUBAGENT_STARTED"),
        [
            "SUBAGENT_STARTED sub-2 plain",
            "ACTIVITY_SNAPSHOT step-2 vymalo.step {\"id\":\"t/a\",\"kind\":\"tool\",\"label\":\"label t/a\",\"path\":[],\"startedAt\":\"2027-01-15T08:00:02Z\",\"state\":\"running\"} @sub-2",
            "STATE_SNAPSHOT working  id:2",
        ]
    );
}

#[test]
fn every_event_of_a_step_says_its_activity_again_under_the_same_id() {
    let mut events = nested();
    events.push(with(
        step(5, "t/c", &["t/o"], Command, Running, Update),
        Some("execute"),
        Some("12 passed"),
    ));
    events.push(step(6, "t/c", &["t/o"], Command, Waiting, Update));
    let frames = support::flatten(&project(&events));
    let snapshots: Vec<&orch_agui_proto::ActivitySnapshotEvent> = frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) if e.message_id.as_str() == "step-4" => {
                Some(e)
            }
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 3, "start and two updates");
    assert!(snapshots.iter().all(|e| e.replace == Some(true)));
    let states: Vec<&str> = snapshots
        .iter()
        .map(|e| e.content["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["running", "running", "waiting"]);
    // the start's time stays, the event's time moves
    let started: Vec<&Value> = snapshots.iter().map(|e| &e.content["startedAt"]).collect();
    assert!(started.iter().all(|s| *s == started[0]));
    assert_ne!(snapshots[0].content["at"], snapshots[2].content["at"]);
    // an icon said once is kept when a later report leaves it out; a detail is the latest's
    let last = &snapshots[2].content;
    assert_eq!(last["icon"], "execute");
    assert!(last.get("detail").is_none());
    assert_eq!(snapshots[1].content["detail"], "12 passed");
}

#[test]
fn a_subagent_step_that_completes_ends_its_subagent_after_its_last_snapshot() {
    let mut events = nested();
    events.push(step(5, "t/c", &["t/o"], Command, Completed, End));
    events.push(step(6, "t/o", &[], Subagent, Completed, End));
    let got = all_lines(&events);
    assert_eq!(
        from(
            &got,
            "ACTIVITY_SNAPSHOT step-4 vymalo.step {\"id\":\"t/c\",\"kind\":\"command\",\"label\":\"label t/c\",\"path\":[\"t/o\"],\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"completed\""
        ),
        [
            "ACTIVITY_SNAPSHOT step-4 vymalo.step {\"id\":\"t/c\",\"kind\":\"command\",\"label\":\"label t/c\",\"path\":[\"t/o\"],\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"completed\"} @sub-step-3  id:5",
            "ACTIVITY_SNAPSHOT step-3 vymalo.step {\"id\":\"t/o\",\"kind\":\"subagent\",\"label\":\"label t/o\",\"path\":[],\"startedAt\":\"2027-01-15T08:00:03Z\",\"state\":\"completed\"} @sub-2",
            "SUBAGENT_FINISHED sub-step-3 success  id:6",
        ]
    );
}

#[test]
fn a_subagent_step_that_fails_is_a_subagent_error_with_its_detail() {
    let mut events = nested();
    events.push(with(
        step(5, "t/o", &[], Subagent, Failed, End),
        None,
        Some("it crashed"),
    ));
    let got = all_lines(&events);
    assert!(got.contains(&"SUBAGENT_ERROR sub-step-3 step_failed \"it crashed\"  id:5".to_owned()));
    // without a detail, the label says what failed
    let mut events = nested();
    events.push(step(5, "t/o", &[], Subagent, Failed, End));
    let got = all_lines(&events);
    assert!(
        got.contains(
            &"SUBAGENT_ERROR sub-step-3 step_failed \"label t/o failed\"  id:5".to_owned()
        )
    );
}

#[test]
fn a_subagent_step_that_is_canceled_finishes_with_a_canceled_result() {
    let mut events = nested();
    events.push(step(5, "t/o", &[], Subagent, Canceled, End));
    let got = all_lines(&events);
    assert!(got.contains(
        &"SUBAGENT_FINISHED sub-step-3 success result={\"status\":\"canceled\"}  id:5".to_owned()
    ));
}

#[test]
fn a_failed_command_is_an_activity_and_the_run_goes_on() {
    let mut events = nested();
    events.push(with(
        step(5, "t/c", &["t/o"], Command, Failed, End),
        None,
        Some("1 failed"),
    ));
    let frames = project(&events);
    let last = lines(frames.last().unwrap());
    assert_eq!(last.len(), 1, "a snapshot and nothing else: {last:?}");
    assert!(last[0].starts_with("ACTIVITY_SNAPSHOT step-4 vymalo.step"));
    assert!(last[0].contains("\"state\":\"failed\"") && last[0].contains("1 failed"));
    // the sub-agent is still open: it can go on
    let all = lines(&support::flatten(&frames));
    assert!(
        !all.iter()
            .any(|l| l.starts_with("SUBAGENT_FINISHED sub-step-3"))
    );
}

#[test]
fn a_subagent_that_ends_ends_what_still_runs_in_it_deepest_first() {
    // o > p > q, all sub-agent steps; o ends while p and q still run
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        step(3, "t/o", &[], Subagent, Running, Start),
        step(4, "t/p", &["t/o"], Subagent, Running, Start),
        step(5, "t/q", &["t/o", "t/p"], Subagent, Running, Start),
        step(6, "t/o", &[], Subagent, Completed, End),
    ];
    let got = all_lines(&events);
    assert_eq!(
        from(&got, "SUBAGENT_STARTED sub-step-4"),
        [
            "SUBAGENT_STARTED sub-step-4 label t/p in sub-step-3",
            "ACTIVITY_SNAPSHOT step-4 vymalo.step {\"id\":\"t/p\",\"kind\":\"subagent\",\"label\":\"label t/p\",\"path\":[\"t/o\"],\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"running\"} @sub-step-3  id:4",
            "SUBAGENT_STARTED sub-step-5 label t/q in sub-step-4",
            "ACTIVITY_SNAPSHOT step-5 vymalo.step {\"id\":\"t/q\",\"kind\":\"subagent\",\"label\":\"label t/q\",\"path\":[\"t/o\",\"t/p\"],\"startedAt\":\"2027-01-15T08:00:05Z\",\"state\":\"running\"} @sub-step-4  id:5",
            "ACTIVITY_SNAPSHOT step-3 vymalo.step {\"id\":\"t/o\",\"kind\":\"subagent\",\"label\":\"label t/o\",\"path\":[],\"startedAt\":\"2027-01-15T08:00:03Z\",\"state\":\"completed\"} @sub-2",
            "SUBAGENT_FINISHED sub-step-5 success result={\"status\":\"canceled\"}",
            "SUBAGENT_FINISHED sub-step-4 success result={\"status\":\"canceled\"}",
            "SUBAGENT_FINISHED sub-step-3 success  id:6",
        ]
    );
}

#[test]
fn a_step_that_starts_again_after_its_end_is_a_new_run_of_it_with_new_ids() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        step(3, "t/a", &[], Subagent, Running, Start),
        step(4, "t/a", &[], Subagent, Failed, End),
        step(5, "t/a", &[], Subagent, Running, Start),
        step(6, "t/a", &[], Subagent, Completed, End),
    ];
    let got = all_lines(&events);
    let started: Vec<_> = got
        .iter()
        .filter(|l| l.starts_with("SUBAGENT_STARTED"))
        .collect();
    assert_eq!(
        started,
        [
            "SUBAGENT_STARTED sub-2 plain",
            "SUBAGENT_STARTED sub-step-3 label t/a in sub-2",
            "SUBAGENT_STARTED sub-step-5 label t/a in sub-2"
        ]
    );
    assert!(
        got.iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT step-5 vymalo.step"))
    );
}

#[test]
fn a_step_that_ends_without_a_start_is_one_activity_with_no_subagent() {
    let got = all_lines(&[
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        step(3, "t/a", &[], Subagent, Completed, End),
    ]);
    assert_eq!(
        from(&got, "ACTIVITY_SNAPSHOT step-3"),
        [
            "ACTIVITY_SNAPSHOT step-3 vymalo.step {\"id\":\"t/a\",\"kind\":\"subagent\",\"label\":\"label t/a\",\"path\":[],\"startedAt\":\"2027-01-15T08:00:03Z\",\"state\":\"completed\"} @sub-2  id:3"
        ]
    );
}

// ---- the invocation closes ---------------------------------------------------------------

#[test]
fn what_is_open_when_the_agent_completes_is_canceled_deepest_first() {
    let mut events = nested();
    events.push(status(5, AgentStatus::Completed, None));
    events.push(thread(6, ThreadState::Done));
    let got = all_lines(&events);
    assert_eq!(
        from(&got, "ACTIVITY_SNAPSHOT evt-5"),
        [
            "ACTIVITY_SNAPSHOT evt-5 vymalo.status {\"status\":\"completed\"} @sub-2",
            // the command first: no spinner stays, and its sub-agent ends after it
            "ACTIVITY_SNAPSHOT step-4 vymalo.step {\"id\":\"t/c\",\"kind\":\"command\",\"label\":\"label t/c\",\"path\":[\"t/o\"],\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"canceled\"} @sub-step-3",
            "ACTIVITY_SNAPSHOT step-3 vymalo.step {\"id\":\"t/o\",\"kind\":\"subagent\",\"label\":\"label t/o\",\"path\":[],\"startedAt\":\"2027-01-15T08:00:03Z\",\"state\":\"canceled\"} @sub-2",
            "SUBAGENT_FINISHED sub-step-3 success result={\"status\":\"canceled\"}",
            "SUBAGENT_FINISHED sub-2 success  id:5",
            "STATE_SNAPSHOT done",
            "RUN_FINISHED run-1 success  id:6",
        ]
    );
}

#[test]
fn a_failed_agent_cancels_its_open_steps_before_the_invocation_errors() {
    let mut events = nested();
    events.push(status(5, AgentStatus::Failed, Some("boom")));
    events.push(thread(6, ThreadState::Failed));
    let got = all_lines(&events);
    let tail = from(&got, "SUBAGENT_FINISHED sub-step-3");
    assert_eq!(
        tail[..2],
        [
            "SUBAGENT_FINISHED sub-step-3 success result={\"status\":\"canceled\"}",
            "SUBAGENT_ERROR sub-2 agent_failed \"boom\"  id:5",
        ]
    );
}

#[test]
fn a_wait_for_the_user_suspends_the_step_subagents_with_no_interrupt_of_their_own() {
    let mut events = nested();
    events.push(status(5, AgentStatus::InputRequired, Some("Allow it?")));
    events.push(thread(6, ThreadState::Blocked));
    let got = all_lines(&events);
    assert_eq!(
        from(&got, "SUBAGENT_FINISHED sub-step-3"),
        [
            "SUBAGENT_FINISHED sub-step-3 suspended[]",
            "SUBAGENT_FINISHED sub-2 suspended[int-5]  id:5",
            "STATE_SNAPSHOT blocked",
            "RUN_FINISHED run-1 interrupt[int-5:input_required @sub-2]  id:6",
        ]
    );
    // the command (not a subagent) has nothing to suspend: it has no frame at all
    assert!(!got.iter().any(|l| l.contains("canceled")));
}

#[test]
fn after_a_suspension_the_steps_say_their_activity_again_under_the_invocation() {
    let mut events = nested();
    events.extend([
        status(5, AgentStatus::InputRequired, Some("Allow it?")),
        thread(6, ThreadState::Blocked),
        user(7, "yes"),
        status(8, AgentStatus::Working, None),
        step(9, "t/c", &["t/o"], Command, Completed, End),
        step(10, "t/o", &[], Subagent, Completed, End),
        status(11, AgentStatus::Completed, None),
        thread(12, ThreadState::Done),
    ]);
    let got = all_lines(&events);
    let second = from(&got, "RUN_STARTED run-7");
    let steps: Vec<_> = second
        .iter()
        .filter(|l| l.contains("vymalo.step") || l.starts_with("SUBAGENT"))
        .collect();
    assert_eq!(
        steps,
        [
            // the invocation reappears under its id; the steps' subagents do not
            "SUBAGENT_STARTED sub-2 plain",
            "ACTIVITY_SNAPSHOT step-4 vymalo.step {\"id\":\"t/c\",\"kind\":\"command\",\"label\":\"label t/c\",\"path\":[\"t/o\"],\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"completed\"} @sub-2  id:9",
            "ACTIVITY_SNAPSHOT step-3 vymalo.step {\"id\":\"t/o\",\"kind\":\"subagent\",\"label\":\"label t/o\",\"path\":[],\"startedAt\":\"2027-01-15T08:00:03Z\",\"state\":\"completed\"} @sub-2  id:10",
            "SUBAGENT_FINISHED sub-2 success  id:11",
        ]
    );
}

#[test]
fn steps_still_open_after_a_resume_are_canceled_when_the_agent_completes() {
    let mut events = nested();
    events.extend([
        status(5, AgentStatus::InputRequired, Some("Allow it?")),
        thread(6, ThreadState::Blocked),
        user(7, "yes"),
        status(8, AgentStatus::Working, None),
        status(9, AgentStatus::Completed, None),
        thread(10, ThreadState::Done),
    ]);
    let frames = support::flatten(&project(&events));
    // the activities say canceled; the suspended subagent is not ended a second time
    assert_eq!(activity(&frames, "step-4")["content"]["state"], "canceled");
    assert_eq!(activity(&frames, "step-3")["content"]["state"], "canceled");
    let got = lines(&frames);
    assert_eq!(
        got.iter()
            .filter(|l| l.starts_with("SUBAGENT_FINISHED sub-step-3"))
            .count(),
        1
    );
}

#[test]
fn a_client_that_joins_while_steps_run_gets_their_subagents_again_parents_first() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        step(3, "t/o", &[], Subagent, Running, Start),
        step(4, "t/p", &["t/o"], Subagent, Running, Start),
        step(5, "t/c", &["t/o", "t/p"], Command, Running, Start),
    ];
    let mut projector = Projector::new(meta());
    for e in &events {
        projector.apply(e, Audience::Viewer);
    }
    let preamble = projector.resume_preamble();
    assert_eq!(
        lines(&preamble),
        [
            "RUN_STARTED run-1",
            "SUBAGENT_STARTED sub-2 plain",
            "SUBAGENT_STARTED sub-step-3 label t/o in sub-2",
            "SUBAGENT_STARTED sub-step-4 label t/p in sub-step-3",
            "STATE_SNAPSHOT working",
        ]
    );
    // and what follows is valid on that connection
    let mut stream = preamble;
    stream.extend(projector.apply(
        &step(6, "t/o", &[], Subagent, Completed, End),
        Audience::Viewer,
    ));
    stream.extend(projector.apply(&status(7, AgentStatus::Completed, None), Audience::Viewer));
    stream.extend(projector.apply(&thread(8, ThreadState::Done), Audience::Viewer));
    verify::check(&stream).unwrap();
}

#[test]
fn a_suspended_step_subagent_is_not_in_the_preamble_of_the_run_that_resumes() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        step(3, "t/o", &[], Subagent, Running, Start),
        status(4, AgentStatus::InputRequired, Some("Allow it?")),
        thread(5, ThreadState::Blocked),
        user(6, "yes"),
        status(7, AgentStatus::Working, None),
    ];
    let mut projector = Projector::new(meta());
    for e in &events {
        projector.apply(e, Audience::Viewer);
    }
    assert_eq!(
        lines(&projector.resume_preamble()),
        [
            "RUN_STARTED run-6",
            "SUBAGENT_STARTED sub-2 plain",
            "STATE_SNAPSHOT working"
        ]
    );
}

#[test]
fn the_next_job_forgets_the_steps_of_the_last() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        step(3, "t/a", &[], Subagent, Running, Start),
        status(4, AgentStatus::Completed, None),
        thread(5, ThreadState::Done),
        user(6, "again"),
        status(7, AgentStatus::Working, None),
        // the same step id in the next job is a new step
        step(8, "t/a", &[], Subagent, Running, Start),
        step(9, "t/a", &[], Subagent, Completed, End),
    ];
    let got = all_lines(&events);
    assert!(
        got.iter()
            .any(|l| l.starts_with("SUBAGENT_STARTED sub-step-8 label t/a in sub-7"))
    );
    assert!(
        got.iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT step-8 vymalo.step"))
    );
}

#[test]
fn a_step_of_an_asked_agent_or_a_relay_is_attributed_to_the_actor_that_reported_it() {
    // the orchestrator's own step: its actor is the calling agent, and the icon names a server
    let relay = with(
        step(3, "tool-1", &[], Tool, Running, Start),
        Some("mcp-server:websearch"),
        None,
    );
    let frames = project(&[user(1, "go"), status(2, AgentStatus::Working, None), relay]);
    let flat = support::flatten(&frames);
    let a = activity(&flat, "step-3");
    assert_eq!(a["content"]["icon"], "mcp-server:websearch");
    assert_eq!(
        a["metadata"],
        json!({"vymalo.actor": {"type": "agent", "name": "plain"}})
    );
}

#[test]
fn what_the_projection_exposes_for_nesting_under_a_step_or_the_agent() {
    let mut projector = Projector::new(meta());
    assert_eq!(projector.invocation_run_id(), None);
    for e in nested() {
        projector.apply(&e, Audience::Viewer);
    }
    assert_eq!(
        projector.invocation_run_id().map(|s| s.as_str()),
        Some("sub-2")
    );
    assert_eq!(
        projector.step_run_id("t/o").map(|s| s.as_str()),
        Some("sub-step-3")
    );
    assert_eq!(
        projector.step_run_id("t/c"),
        None,
        "a command has no subagent"
    );
    assert_eq!(projector.step_run_id("nope"), None);
}

#[test]
fn a_message_open_while_a_step_says_something_stays_open() {
    // a partial message is not interrupted by a step: the activity goes in between, the text
    // goes on, and no resume point falls inside the message
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        ev(
            3,
            plain(),
            EventBody::AgentMessage(AgentMessageData {
                text: "Wor".to_owned(),
                message_id: "m".to_owned(),
                is_final: false,
                purpose: None,
                via: None,
            }),
        ),
        step(4, "t/a", &[], Tool, Running, Start),
        ev(
            5,
            plain(),
            EventBody::AgentMessage(AgentMessageData {
                text: "Working".to_owned(),
                message_id: "m".to_owned(),
                is_final: true,
                purpose: None,
                via: None,
            }),
        ),
    ];
    let frames = project(&events);
    assert!(frames[3].iter().all(|f| f.resume_id.is_none()));
    assert!(frames[4].last().unwrap().resume_id.is_some());
}

#[test]
fn the_ids_of_the_step_activities_are_ids_the_thread_holds() {
    // a client sends the messages it holds back with its next run: an id the thread does not
    // know is a 422, so a step's activity id is one it knows
    let mut projector = Projector::new(meta());
    for e in nested() {
        projector.apply(&e, Audience::Viewer);
    }
    let orch_agui_projection::ThreadView::Known(known) =
        projector.view(&UserId::new("alice@example.com"))
    else {
        panic!("a known thread");
    };
    assert!(known.message_ids.contains("step-3") && known.message_ids.contains("step-4"));
}

#[test]
fn a_connect_stream_that_joins_in_the_middle_of_a_step_run_is_whole() {
    use orch_agui_projection::{Connect, Follow};
    let mut events = nested();
    events.extend([
        step(5, "t/c", &["t/o"], Command, Failed, End),
        step(6, "t/o", &[], Subagent, Completed, End),
        status(7, AgentStatus::Completed, None),
        thread(8, ThreadState::Done),
    ]);
    let full = support::flatten(&project(&events));
    // every cursor that is a resume point, joined by a client that holds the stream up to it
    for cursor in 2..=8_i64 {
        let mut connect = Connect::new(meta(), cursor, 8, Follow::Forever);
        let mut got = Vec::new();
        for e in &events {
            got.extend(connect.feed(e));
        }
        if got.is_empty() {
            continue;
        }
        verify::check(&got).unwrap_or_else(|e| panic!("cursor {cursor}: {e}\n{:#?}", lines(&got)));
        assert!(
            got.len() <= full.len() + 8,
            "the preamble re-opens at most the open subagents"
        );
    }
}

/// ADR 0030: the input is logged with the start and the output with the end, and every snapshot
/// of the step says what the step has by then: the end's carries both, `ioDropped` only when the
/// log says so, and none of them is there when the step had none.
#[test]
fn a_steps_activity_carries_its_input_and_output() {
    let mut start = step(3, "t/c", &[], Tool, Running, Start);
    let mut end = step(4, "t/c", &[], Tool, Failed, End);
    if let EventBody::AgentStep(d) = &mut start.body {
        d.input = serde_json::json!({"query": "node 24", "limit": 3})
            .as_object()
            .cloned();
    }
    if let EventBody::AgentStep(d) = &mut end.body {
        d.output = Some(orch_core::StepOutput {
            text: "no such host".to_owned(),
            truncated: true,
            bytes: Some(9000),
            error: true,
        });
    }
    let plain = step(5, "t/d", &[], Tool, Completed, End);
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        start,
        end,
        plain,
    ];
    let frames = project(&events);
    let flat = support::flatten(&frames);
    let first = &frames[2]
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) => Some(e.content.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        first["input"],
        serde_json::json!({"query": "node 24", "limit": 3})
    );
    assert!(first.get("output").is_none());
    let last = activity(&flat, "step-3");
    assert_eq!(last["content"]["state"], "failed");
    assert_eq!(
        last["content"]["input"],
        serde_json::json!({"query": "node 24", "limit": 3}),
        "the end says the input again"
    );
    assert_eq!(
        last["content"]["output"],
        serde_json::json!({"text": "no such host", "truncated": true, "bytes": 9000, "error": true})
    );
    assert!(last["content"].get("ioDropped").is_none());
    let bare = activity(&flat, "step-5");
    assert!(bare["content"].get("input").is_none());
    assert!(bare["content"].get("output").is_none());
}
