//! A rename (`thread_titled`, `docs/api/agui.md`, "Titles"), row by row, on hand-written logs: the
//! title is part of every `STATE_SNAPSHOT`, so a rename is said as one, inside a run or, when
//! nothing is going on, as a run of its own that holds only that snapshot.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, Event, EventBody, ThreadState, ThreadStateData,
    ThreadTitledData, Timestamp, TitledBy, UserId, UserMessageData,
};
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

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

fn user(seq: i64, text: &str) -> Event {
    ev(
        seq,
        Actor::user(&alice()),
        EventBody::UserMessage(UserMessageData::new(text)),
    )
}

fn status(seq: i64, status: AgentStatus, detail: Option<&str>) -> Event {
    ev(
        seq,
        Actor::agent(&AgentId::new("plain"), None),
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

fn rename(seq: i64, title: &str) -> Event {
    ev(
        seq,
        Actor::user(&alice()),
        EventBody::ThreadTitled(ThreadTitledData {
            title: title.to_owned(),
            source: TitledBy::User,
        }),
    )
}

/// Projects `events` for a viewer, checking the stream, one entry per event.
fn project(events: &[Event]) -> Vec<Vec<Frame>> {
    let mut projector = Projector::new(meta());
    let frames: Vec<Vec<Frame>> = events
        .iter()
        .map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    verify::check(&support::flatten(&frames)).unwrap();
    frames
}

fn snapshot_titles(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::StateSnapshot(s) => {
                s.snapshot["thread"]["title"].as_str().map(str::to_owned)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn inside_a_run_a_rename_is_a_state_snapshot_with_the_new_title() {
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        rename(3, "Fix the build"),
    ];
    let frames = project(&events);
    assert_eq!(
        lines(&frames[2]),
        ["STATE_SNAPSHOT working  id:3"],
        "one snapshot, a resume point"
    );
    assert_eq!(snapshot_titles(&frames[2]), ["Fix the build"]);
    // what came before said the title the thread had
    assert_eq!(snapshot_titles(&frames[0]), ["a thread"]);
}

#[test]
fn after_a_rename_every_later_snapshot_says_the_new_title() {
    let events = [
        user(1, "go"),
        rename(2, "Fix the build"),
        status(3, AgentStatus::Working, None),
        status(4, AgentStatus::Completed, None),
        thread(5, ThreadState::Done),
    ];
    let frames = support::flatten(&project(&events));
    let titles = snapshot_titles(&frames);
    assert_eq!(titles.first().map(String::as_str), Some("a thread"));
    assert!(
        titles[1..].iter().all(|t| t == "Fix the build"),
        "{titles:?}"
    );
    assert_eq!(titles.last().map(String::as_str), Some("Fix the build"));
}

#[test]
fn a_rename_of_a_finished_thread_is_a_run_of_its_own_that_holds_one_snapshot() {
    let events = [
        user(1, "echo"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::Completed, None),
        thread(4, ThreadState::Done),
        rename(5, "Fix the build"),
    ];
    let frames = project(&events);
    assert_eq!(
        lines(&frames[4]),
        [
            "RUN_STARTED run-5",
            "STATE_SNAPSHOT done",
            "RUN_FINISHED run-5 success  id:5"
        ]
    );
    assert_eq!(snapshot_titles(&frames[4]), ["Fix the build"]);
}

#[test]
fn a_rename_of_a_cancelled_thread_ends_as_the_thread_ended() {
    let events = [
        user(1, "slow"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::Canceled, Some("canceled")),
        thread(4, ThreadState::Cancelled),
        rename(5, "Mine"),
    ];
    let frames = project(&events);
    assert_eq!(
        lines(&frames[4]),
        [
            "RUN_STARTED run-5",
            "STATE_SNAPSHOT cancelled",
            "RUN_FINISHED run-5 cancelled  id:5"
        ]
    );
}

#[test]
fn a_rename_of_a_thread_that_waits_for_the_user_says_the_same_wait_again() {
    let events = [
        user(1, "ask"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::InputRequired, Some("Which branch?")),
        thread(4, ThreadState::Blocked),
        rename(5, "Mine"),
    ];
    let frames = project(&events);
    let got = lines(&frames[4]);
    let before = lines(&frames[3]);
    assert_eq!(got.first().map(String::as_str), Some("RUN_STARTED run-5"));
    assert!(
        got.last()
            .unwrap()
            .starts_with("RUN_FINISHED run-5 interrupt["),
        "{got:?}"
    );
    // the interrupt is the one the thread was waiting on, so an answer still resumes it
    let interrupt = |lines: &[String]| {
        let line = lines
            .iter()
            .find(|l| l.starts_with("RUN_FINISHED"))
            .unwrap();
        line[line.find("interrupt[").unwrap()..]
            .split("  id:")
            .next()
            .unwrap()
            .to_owned()
    };
    assert_eq!(interrupt(&got), interrupt(&before));
}

#[test]
fn a_rename_of_a_failed_thread_says_the_failure_again_not_a_new_one() {
    let events = [
        user(1, "fail"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::Failed, Some("boom")),
        thread(4, ThreadState::Failed),
        rename(5, "Mine"),
    ];
    let frames = project(&events);
    let got = lines(&frames[4]);
    assert_eq!(got.first().map(String::as_str), Some("RUN_STARTED run-5"));
    let error = |lines: &[String]| {
        lines
            .iter()
            .find(|l| l.starts_with("RUN_ERROR"))
            .and_then(|l| l.split("  id:").next())
            .map(str::to_owned)
            .unwrap()
    };
    assert_eq!(error(&got), error(&lines(&frames[3])), "{got:?}");
}

#[test]
fn a_rename_is_a_resume_point_only_when_no_message_is_open() {
    // an agent message that is still open when the rename arrives: the frame is not a resume
    // point, the message's own end is
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        ev(
            3,
            Actor::agent(&AgentId::new("plain"), None),
            EventBody::AgentMessage(orch_core::AgentMessageData {
                text: "Wor".to_owned(),
                message_id: "m1".to_owned(),
                is_final: false,
                purpose: None,
                via: None,
            }),
        ),
        rename(4, "Mine"),
    ];
    let frames = project(&events);
    assert!(
        frames[3].iter().all(|f| f.resume_id.is_none()),
        "{:?}",
        lines(&frames[3])
    );
}

#[test]
fn a_second_rename_is_another_snapshot() {
    let events = [
        user(1, "go"),
        rename(2, "One"),
        rename(3, "Two"),
        status(4, AgentStatus::Working, None),
    ];
    let frames = project(&events);
    assert_eq!(snapshot_titles(&frames[1]), ["One"]);
    assert_eq!(snapshot_titles(&frames[2]), ["Two"]);
}

#[test]
fn a_rename_never_changes_the_transcript() {
    // no message, activity or subagent frame comes of it
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        rename(3, "x"),
    ];
    let frames = project(&events);
    assert!(
        frames[2]
            .iter()
            .all(|f| matches!(f.event, orch_agui_proto::Event::StateSnapshot(_)))
    );
}
