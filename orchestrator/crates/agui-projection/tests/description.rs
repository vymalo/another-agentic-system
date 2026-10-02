//! A description (`thread_described`, `docs/api/agui.md`, "Descriptions"), row by row, on
//! hand-written logs: the description is part of every `STATE_SNAPSHOT` that has one, so a change
//! is said as one, inside a run or, when nothing is going on, as a run of its own that holds only
//! that snapshot, exactly as a rename is.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, DescribedBy, Event, EventBody, ForkKind,
    ForkSource, JobStartedData, ThreadDescribedData, ThreadForkedData, ThreadState,
    ThreadStateData, Timestamp, UserId, UserMessageData,
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

fn status(seq: i64, status: AgentStatus) -> Event {
    ev(
        seq,
        Actor::agent(&AgentId::new("plain"), None),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: None,
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

/// The model describes the thread.
fn described(seq: i64, description: &str) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::ThreadDescribed(ThreadDescribedData {
            description: description.to_owned(),
            source: DescribedBy::Model,
        }),
    )
}

/// A person writes (or, with nothing, clears) the description.
fn written(seq: i64, description: &str) -> Event {
    ev(
        seq,
        Actor::user(&alice()),
        EventBody::ThreadDescribed(ThreadDescribedData {
            description: description.to_owned(),
            source: DescribedBy::User,
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

/// The `thread.description` of each snapshot of `frames`: `None` for one that says none.
fn snapshot_descriptions(frames: &[Frame]) -> Vec<Option<String>> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::StateSnapshot(s) => Some(
                s.snapshot["thread"]
                    .get("description")
                    .map(|d| d.as_str().unwrap().to_owned()),
            ),
            _ => None,
        })
        .collect()
}

fn some(text: &str) -> Vec<Option<String>> {
    vec![Some(text.to_owned())]
}

#[test]
fn a_thread_with_no_description_says_none_in_its_snapshots() {
    let frames = project(&[user(1, "go"), status(2, AgentStatus::Working)]);
    for entry in &frames {
        assert!(snapshot_descriptions(entry).iter().all(Option::is_none));
    }
}

#[test]
fn inside_a_run_a_description_is_a_state_snapshot_with_the_new_description() {
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Working),
        described(3, "Fixing the build."),
    ];
    let frames = project(&events);
    assert_eq!(
        lines(&frames[2]),
        ["STATE_SNAPSHOT working  id:3"],
        "one snapshot, a resume point"
    );
    assert_eq!(snapshot_descriptions(&frames[2]), some("Fixing the build."));
}

#[test]
fn after_a_description_every_later_snapshot_says_it() {
    let events = [
        user(1, "go"),
        described(2, "Fixing the build."),
        status(3, AgentStatus::Working),
        status(4, AgentStatus::Completed),
        thread(5, ThreadState::Done),
    ];
    let frames = support::flatten(&project(&events));
    let all = snapshot_descriptions(&frames);
    assert_eq!(all.first(), Some(&None), "what came before said none");
    assert!(
        all[1..]
            .iter()
            .all(|d| d.as_deref() == Some("Fixing the build.")),
        "{all:?}"
    );
}

#[test]
fn a_description_of_a_finished_thread_is_a_run_of_its_own_that_holds_one_snapshot() {
    let events = [
        user(1, "echo"),
        status(2, AgentStatus::Working),
        status(3, AgentStatus::Completed),
        thread(4, ThreadState::Done),
        described(5, "The person said hello."),
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
    assert_eq!(
        snapshot_descriptions(&frames[4]),
        some("The person said hello.")
    );
}

#[test]
fn a_description_of_a_cancelled_or_waiting_thread_ends_as_the_thread_ended() {
    let cancelled = [
        user(1, "slow"),
        status(2, AgentStatus::Working),
        status(3, AgentStatus::Canceled),
        thread(4, ThreadState::Cancelled),
        described(5, "Cancelled work."),
    ];
    assert_eq!(
        lines(&project(&cancelled)[4]),
        [
            "RUN_STARTED run-5",
            "STATE_SNAPSHOT cancelled",
            "RUN_FINISHED run-5 cancelled  id:5"
        ]
    );
    let waiting = [
        user(1, "ask"),
        status(2, AgentStatus::Working),
        ev(
            3,
            Actor::agent(&AgentId::new("plain"), None),
            EventBody::AgentStatus(AgentStatusData {
                status: AgentStatus::InputRequired,
                detail: Some("Which branch?".to_owned()),
            }),
        ),
        thread(4, ThreadState::Blocked),
        described(5, "Choosing a branch."),
    ];
    let got = lines(&project(&waiting)[4]);
    assert_eq!(got.first().map(String::as_str), Some("RUN_STARTED run-5"));
    assert!(
        got.last()
            .unwrap()
            .starts_with("RUN_FINISHED run-5 interrupt["),
        "{got:?}"
    );
}

#[test]
fn a_person_clearing_it_is_a_snapshot_with_no_description() {
    let events = [
        user(1, "go"),
        described(2, "Fixing the build."),
        written(3, ""),
        status(4, AgentStatus::Working),
    ];
    let frames = project(&events);
    assert_eq!(snapshot_descriptions(&frames[1]), some("Fixing the build."));
    assert_eq!(snapshot_descriptions(&frames[2]), vec![None]);
    assert_eq!(snapshot_descriptions(&frames[3]), vec![None]);
}

#[test]
fn a_person_writing_after_the_model_is_the_last_one_standing() {
    let events = [
        user(1, "go"),
        described(2, "The model's."),
        written(3, "The person's."),
        status(4, AgentStatus::Working),
    ];
    let frames = project(&events);
    assert_eq!(snapshot_descriptions(&frames[1]), some("The model's."));
    assert_eq!(snapshot_descriptions(&frames[2]), some("The person's."));
    assert_eq!(snapshot_descriptions(&frames[3]), some("The person's."));
}

#[test]
fn a_description_never_changes_the_transcript_and_is_a_resume_point_only_when_no_message_is_open() {
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Working),
        described(3, "x"),
        ev(
            4,
            Actor::agent(&AgentId::new("plain"), None),
            EventBody::AgentMessage(orch_core::AgentMessageData {
                text: "Wor".to_owned(),
                message_id: "m1".to_owned(),
                is_final: false,
                purpose: None,
                via: None,
            }),
        ),
        described(5, "y"),
    ];
    let frames = project(&events);
    assert!(
        frames[2]
            .iter()
            .all(|f| matches!(f.event, orch_agui_proto::Event::StateSnapshot(_)))
    );
    assert!(
        frames[4].iter().all(|f| f.resume_id.is_none()),
        "{:?}",
        lines(&frames[4])
    );
}

#[test]
fn a_fork_has_the_parents_description_from_its_marker_on() {
    let events = [
        user(1, "fix the loop"),
        status(2, AgentStatus::Completed),
        thread(3, ThreadState::Done),
        ev(
            4,
            Actor::user(&alice()),
            EventBody::ThreadForked(ThreadForkedData {
                from: ForkSource {
                    thread_id: thread_id(),
                    seq: 3,
                },
                kind: ForkKind::Fork,
                title: "Fix the loop".to_owned(),
                description: Some("Fixing a redirect loop.".to_owned()),
                target: orch_core::AgentTarget {
                    agent_id: AgentId::new("plain"),
                    release: None,
                },
            }),
        ),
        // the fork's own life: the next message starts job 2
        user(5, "and the footer"),
        ev(
            6,
            Actor::system(),
            EventBody::JobStarted(JobStartedData { job: 2 }),
        ),
    ];
    let frames = project(&events);
    assert_eq!(
        snapshot_descriptions(&frames[3]),
        some("Fixing a redirect loop.")
    );
    assert!(
        snapshot_descriptions(&frames[4])
            .iter()
            .all(|d| d.as_deref() == Some("Fixing a redirect loop.")),
        "the next message of the fork says it too"
    );
    // a fork of a thread that had none says none
    let mut bare = events.clone();
    if let EventBody::ThreadForked(d) = &mut bare[3].body {
        d.description = None;
    }
    let frames = project(&bare);
    assert_eq!(snapshot_descriptions(&frames[3]), vec![None]);
}
