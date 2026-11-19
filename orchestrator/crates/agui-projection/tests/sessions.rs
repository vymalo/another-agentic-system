//! Child sessions (ADR 0072, `docs/api/agui.md`, "Child sessions as subagents"), row by row, on
//! hand-written logs: an opened session is a subagent of the agent's invocation and a
//! `vymalo.session` card; a report ends the round (the subagent finishes, or errors for a failed
//! round) and a later round opens the same subagent again; a close ends the card and the subagent
//! if it is open. A child thread says the round its `job_started` follows.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, ChildSessionClosedData, ChildSessionOpenedData,
    ChildSessionReportedData, Event, EventBody, JobRound, JobStartedData, SessionBy,
    SessionOutcome, ThreadId, ThreadState, ThreadStateData, Timestamp, UserId, UserMessageData,
};
use serde_json::{Value, json};
use support::log::{meta, thread_id};
use support::{lines, verify};

const CHILD: &str = "00000000-0000-7000-8000-000000000003";

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

fn status(seq: i64, status: AgentStatus) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: None,
            tier: None,
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

fn child() -> ThreadId {
    CHILD.parse().unwrap()
}

fn opened(seq: i64, session: &str, keep_open: bool) -> Event {
    ev(
        seq,
        plain(),
        EventBody::ChildSessionOpened(ChildSessionOpenedData {
            session: session.to_owned(),
            thread: child(),
            agent: "helper".to_owned(),
            description: "find the data".to_owned(),
            keep_open,
            by: Some(SessionBy::Agent),
            round: Some(1),
        }),
    )
}

fn reported(seq: i64, session: &str, round: u32, report: SessionOutcome) -> Event {
    ev(
        seq,
        plain(),
        EventBody::ChildSessionReported(ChildSessionReportedData {
            session: session.to_owned(),
            thread: child(),
            round,
            report,
        }),
    )
}

fn closed(seq: i64, session: &str) -> Event {
    ev(
        seq,
        plain(),
        EventBody::ChildSessionClosed(ChildSessionClosedData {
            session: session.to_owned(),
            thread: child(),
        }),
    )
}

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

fn from(got: &[String], marker: &str) -> Vec<String> {
    got.iter()
        .skip_while(|l| !l.starts_with(marker))
        .cloned()
        .collect()
}

/// The `vymalo.session` card as its last snapshot said it.
fn card(frames: &[Frame]) -> Value {
    frames
        .iter()
        .rev()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) if e.activity_type == "vymalo.session" => {
                Some(serde_json::to_value(e).unwrap())
            }
            _ => None,
        })
        .expect("no vymalo.session card")
}

/// The agent works (seq 2, its invocation `sub-2`) and opens session `s1` at seq 3.
fn working() -> Vec<Event> {
    vec![
        user(1, "go"),
        status(2, AgentStatus::Working),
        opened(3, "s1", true),
    ]
}

#[test]
fn an_opened_session_is_a_subagent_of_the_invocation_and_a_card_that_says_what_it_is_for() {
    let events = working();
    let got = all_lines(&events);
    let tail = from(&got, "SUBAGENT_STARTED sub-session-3");
    assert_eq!(tail[0], "SUBAGENT_STARTED sub-session-3 helper in sub-2");
    assert!(
        tail[1].starts_with("ACTIVITY_SNAPSHOT session-3 vymalo.session {"),
        "{tail:#?}"
    );
    assert!(tail[1].ends_with("  id:3"), "the card is the resume point");
    assert_eq!(tail.len(), 2);

    let frames = support::flatten(&project(&events));
    let started = frames
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::SubagentStarted(e)
                if e.subagent_run_id.as_str() == "sub-session-3" =>
            {
                Some(serde_json::to_value(e).unwrap())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(started["description"], "find the data");
    assert_eq!(started["metadata"]["vymalo.actor"]["name"], "plain");
    assert_eq!(started["metadata"]["vymalo.at"], "2027-01-15T08:00:03Z");
    assert_eq!(started["metadata"]["vymalo.childThread"], CHILD);
    assert_eq!(started["metadata"]["vymalo.session"], "s1");

    let snapshot = card(&frames);
    assert_eq!(snapshot["messageId"], "session-3");
    assert_eq!(snapshot["replace"], true);
    assert_eq!(snapshot["metadata"]["vymalo.actor"]["name"], "plain");
    assert_eq!(
        snapshot["content"],
        json!({
            "session": "s1",
            "threadId": CHILD,
            "agent": "helper",
            "description": "find the data",
            "keepOpen": true,
            "state": "working",
            "round": 1,
            "at": "2027-01-15T08:00:03Z",
        })
    );
}

#[test]
fn a_report_updates_the_card_and_finishes_the_subagent_with_the_round_and_how_it_ended() {
    for outcome in [
        SessionOutcome::Done,
        SessionOutcome::Blocked,
        SessionOutcome::Canceled,
    ] {
        let mut events = working();
        events.push(reported(4, "s1", 1, outcome));
        let frames = project(&events);
        let got = lines(&support::flatten(&frames));
        let report = serde_json::to_value(outcome).unwrap();
        assert_eq!(
            got.last().unwrap(),
            &format!(
                "SUBAGENT_FINISHED sub-session-3 success result={{\"report\":{report},\"round\":1}}  id:4"
            )
        );
        // the card comes first
        let tail = from(&got, "ACTIVITY_SNAPSHOT session-3");
        assert_eq!(tail.len(), 3, "{tail:#?}");
        let snapshot = card(&support::flatten(&frames));
        assert_eq!(snapshot["content"]["state"], "idle");
        assert_eq!(snapshot["content"]["report"], report);
        assert_eq!(snapshot["content"]["round"], 1);
        assert_eq!(snapshot["content"]["at"], "2027-01-15T08:00:04Z");
    }
}

#[test]
fn a_failed_round_is_a_subagent_error_with_a_code_and_the_card_still_says_it() {
    let mut events = working();
    events.push(reported(4, "s1", 1, SessionOutcome::Failed));
    let frames = project(&events);
    let got = lines(&support::flatten(&frames));
    assert_eq!(
        got.last().unwrap(),
        "SUBAGENT_ERROR sub-session-3 session_failed \"helper failed round 1\"  id:4"
    );
    let snapshot = card(&support::flatten(&frames));
    assert_eq!(snapshot["content"]["report"], "failed");
    assert_eq!(snapshot["content"]["state"], "idle");
}

#[test]
fn a_later_round_opens_the_subagent_again_before_it_says_the_round() {
    let mut events = working();
    events.push(reported(4, "s1", 1, SessionOutcome::Done));
    events.push(reported(5, "s1", 2, SessionOutcome::Failed));
    events.push(reported(6, "s1", 3, SessionOutcome::Done));
    let frames = project(&events);
    // an id is used once in a run (the reference client refuses a second start), so the rounds
    // after the first, in the run that already ended the session's own id, have ids of their own
    let second = lines(&frames[4]);
    assert_eq!(
        second[0],
        "SUBAGENT_STARTED sub-session-3-r2 helper in sub-2"
    );
    assert!(second[1].starts_with("ACTIVITY_SNAPSHOT session-3 vymalo.session"));
    assert_eq!(
        second[2],
        "SUBAGENT_ERROR sub-session-3-r2 session_failed \"helper failed round 2\"  id:5"
    );
    let third = lines(&frames[5]);
    assert_eq!(
        third[0],
        "SUBAGENT_STARTED sub-session-3-r3 helper in sub-2"
    );
    assert_eq!(
        third[2],
        "SUBAGENT_FINISHED sub-session-3-r3 success result={\"report\":\"done\",\"round\":3}  id:6"
    );
    let snapshot = card(&support::flatten(&frames));
    assert_eq!(snapshot["content"]["round"], 3);
}

#[test]
fn a_close_sets_the_card_and_finishes_a_subagent_that_is_still_open() {
    // still open: the first round never reported
    let mut events = working();
    events.push(closed(4, "s1"));
    let frames = project(&events);
    let got = lines(&support::flatten(&frames));
    assert_eq!(
        got.last().unwrap(),
        "SUBAGENT_FINISHED sub-session-3 success result={\"state\":\"closed\"}  id:4"
    );
    assert_eq!(
        card(&support::flatten(&frames))["content"]["state"],
        "closed"
    );

    // already finished by its report: the card alone
    let mut events = working();
    events.push(reported(4, "s1", 1, SessionOutcome::Done));
    events.push(closed(5, "s1"));
    let frames = project(&events);
    let last = lines(&frames[4]);
    assert_eq!(last.len(), 1, "{last:#?}");
    assert!(last[0].starts_with("ACTIVITY_SNAPSHOT session-3 vymalo.session"));
    assert!(last[0].ends_with("  id:5"));
    let snapshot = card(&support::flatten(&frames));
    assert_eq!(snapshot["content"]["state"], "closed");
    assert_eq!(
        snapshot["content"]["report"], "done",
        "the last round stays"
    );
}

#[test]
fn a_session_event_while_no_run_is_open_is_a_run_of_its_own() {
    // the thread finished; a background helper reports afterwards
    let mut events = working();
    events.push(reported(4, "s1", 1, SessionOutcome::Done));
    events.push(status(5, AgentStatus::Completed));
    events.push(thread(6, ThreadState::Done));
    events.push(reported(7, "s1", 2, SessionOutcome::Done));
    events.push(closed(8, "s1"));
    let frames = project(&events);
    let late = lines(&frames[6]);
    assert_eq!(late[0], "RUN_STARTED run-7");
    // a new run: the session's own id again
    assert!(late.contains(&"SUBAGENT_STARTED sub-session-3 helper in sub-7".to_owned()));
    assert!(
        late.iter().any(|l| l.starts_with("STATE_SNAPSHOT")),
        "the run says the thread's state"
    );
    assert!(
        late.last().unwrap().starts_with("RUN_FINISHED"),
        "{late:#?}"
    );
    // and a close on its own is a run too, with the card and nothing else
    let close = lines(&frames[7]);
    assert_eq!(close[0], "RUN_STARTED run-8");
    assert!(
        close
            .iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT session-3"))
    );
    assert!(close.last().unwrap().starts_with("RUN_FINISHED"));
    assert!(!close.iter().any(|l| l.starts_with("SUBAGENT")));
}

#[test]
fn an_event_seen_twice_changes_nothing() {
    let mut events = working();
    events.push(reported(4, "s1", 1, SessionOutcome::Done));
    events.push(closed(5, "s1"));
    let mut projector = Projector::new(meta());
    let mut said = Vec::new();
    for e in &events {
        said.extend(projector.apply(e, Audience::Viewer));
    }
    verify::check(&said).unwrap();
    for again in &events[2..] {
        let frames = projector.apply(again, Audience::Viewer);
        assert!(frames.is_empty(), "{:?}", lines(&frames));
    }
    // and a report of an earlier round after a later one
    let mut projector = Projector::new(meta());
    let mut log = working();
    log.push(reported(4, "s1", 2, SessionOutcome::Done));
    for e in &log {
        projector.apply(e, Audience::Viewer);
    }
    assert!(
        projector
            .apply(
                &reported(5, "s1", 1, SessionOutcome::Failed),
                Audience::Viewer
            )
            .is_empty()
    );
}

#[test]
fn a_session_the_log_never_opened_says_nothing() {
    let mut projector = Projector::new(meta());
    assert!(
        projector
            .apply(
                &reported(2, "ghost", 1, SessionOutcome::Done),
                Audience::Viewer
            )
            .is_empty()
    );
    assert!(
        projector
            .apply(&closed(3, "ghost"), Audience::Viewer)
            .is_empty()
    );
}

#[test]
fn a_session_still_open_when_the_invocation_ends_is_suspended_and_not_canceled() {
    let mut events = working();
    events.push(status(4, AgentStatus::Completed));
    events.push(thread(5, ThreadState::Done));
    let got = all_lines(&events);
    let at = got
        .iter()
        .position(|l| l == "SUBAGENT_FINISHED sub-session-3 suspended[]")
        .unwrap_or_else(|| panic!("{got:#?}"));
    let invocation = got
        .iter()
        .position(|l| l.starts_with("SUBAGENT_FINISHED sub-2"))
        .unwrap();
    assert!(
        at < invocation,
        "it ends before the invocation that holds it"
    );
    // its report, later, opens it again under a new invocation
    events.push(reported(6, "s1", 1, SessionOutcome::Done));
    let got = all_lines(&events);
    assert!(got.contains(&"SUBAGENT_STARTED sub-session-3 helper in sub-6".to_owned()));
}

#[test]
fn a_viewer_who_joins_mid_run_is_told_of_the_open_sessions() {
    let mut projector = Projector::new(meta());
    for e in &working() {
        projector.apply(e, Audience::Viewer);
    }
    let preamble = lines(&projector.resume_preamble());
    assert!(
        preamble.contains(&"SUBAGENT_STARTED sub-session-3 helper in sub-2".to_owned()),
        "{preamble:#?}"
    );
    projector.apply(
        &reported(4, "s1", 1, SessionOutcome::Done),
        Audience::Viewer,
    );
    let preamble = lines(&projector.resume_preamble());
    assert!(!preamble.iter().any(|l| l.contains("sub-session-3")));
}

#[test]
fn the_job_of_a_child_thread_says_the_round_it_follows() {
    let started = |round: Option<JobRound>| {
        ev(
            2,
            Actor::system(),
            EventBody::JobStarted(JobStartedData { job: 1, round }),
        )
    };
    for (round, want) in [
        (None, json!({"job": 1})),
        (
            Some(JobRound {
                number: 2,
                by: Some(SessionBy::Agent),
            }),
            json!({"job": 1, "round": {"number": 2, "by": "agent"}}),
        ),
        (
            Some(JobRound {
                number: 1,
                by: None,
            }),
            json!({"job": 1, "round": {"number": 1}}),
        ),
    ] {
        let frames = project(&[started(round)]);
        let mut content = card_of(&support::flatten(&frames), "vymalo.job");
        content.as_object_mut().unwrap().remove("at");
        assert_eq!(content, want);
    }
}

fn card_of(frames: &[Frame], activity_type: &str) -> Value {
    frames
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) if e.activity_type == activity_type => {
                Some(serde_json::to_value(&e.content).unwrap())
            }
            _ => None,
        })
        .unwrap()
}
