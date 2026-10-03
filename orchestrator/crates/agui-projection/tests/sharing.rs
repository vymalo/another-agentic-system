//! Sharing (`thread_shared`, `thread_unshared`, ADR 0040) says nothing to a screen: who may read a
//! thread is not part of the transcript, so the two events produce no frame, in a run or outside
//! one, and an `error` before them still explains the `thread_state` that follows.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, ErrorData, Event, EventBody, ShareLevel,
    ThreadSharedData, ThreadState, ThreadStateData, ThreadUnsharedData, Timestamp, UserId,
    UserMessageData,
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

fn owner() -> Actor {
    Actor::user(&UserId::new("alice@example.com"))
}

fn shared(seq: i64) -> Event {
    ev(
        seq,
        owner(),
        EventBody::ThreadShared(ThreadSharedData {
            visibility: ShareLevel::Public,
            nonce_sha256: "00".repeat(32),
        }),
    )
}

fn unshared(seq: i64) -> Event {
    ev(
        seq,
        owner(),
        EventBody::ThreadUnshared(ThreadUnsharedData {}),
    )
}

fn project(events: &[Event]) -> Vec<Vec<Frame>> {
    let mut projector = Projector::new(meta());
    let frames: Vec<Vec<Frame>> = events
        .iter()
        .map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    verify::check(&support::flatten(&frames)).unwrap();
    frames
}

fn done_thread() -> Vec<Event> {
    vec![
        ev(
            1,
            owner(),
            EventBody::UserMessage(UserMessageData::new("hi")),
        ),
        ev(
            2,
            Actor::agent(&AgentId::new("plain"), None),
            EventBody::AgentStatus(AgentStatusData {
                status: AgentStatus::Completed,
                detail: Some("hello".into()),
            }),
        ),
        ev(
            3,
            Actor::system(),
            EventBody::ThreadState(ThreadStateData {
                state: ThreadState::Done,
            }),
        ),
    ]
}

#[test]
fn a_share_and_an_unshare_say_nothing_outside_a_run() {
    let mut log = done_thread();
    log.push(shared(4));
    log.push(unshared(5));
    let frames = project(&log);
    assert!(
        frames[3].is_empty(),
        "thread_shared: {:?}",
        lines(&frames[3])
    );
    assert!(
        frames[4].is_empty(),
        "thread_unshared: {:?}",
        lines(&frames[4])
    );
    // and they are not a resume point: nothing to resume from
    assert!(
        frames[3]
            .iter()
            .chain(&frames[4])
            .all(|f| f.resume_id.is_none())
    );
}

#[test]
fn a_share_inside_a_run_says_nothing_and_the_run_goes_on() {
    let log = vec![
        ev(
            1,
            owner(),
            EventBody::UserMessage(UserMessageData::new("work")),
        ),
        shared(2),
        ev(
            3,
            Actor::agent(&AgentId::new("plain"), None),
            EventBody::AgentStatus(AgentStatusData {
                status: AgentStatus::Working,
                detail: None,
            }),
        ),
        unshared(4),
        ev(
            5,
            Actor::agent(&AgentId::new("plain"), None),
            EventBody::AgentStatus(AgentStatusData {
                status: AgentStatus::Completed,
                detail: Some("done".into()),
            }),
        ),
        ev(
            6,
            Actor::system(),
            EventBody::ThreadState(ThreadStateData {
                state: ThreadState::Done,
            }),
        ),
    ];
    let frames = project(&log);
    assert!(frames[1].is_empty() && frames[3].is_empty());
    // the same frames as the log without the two events
    let plain: Vec<Event> = log
        .iter()
        .filter(|e| {
            !matches!(
                e.body,
                EventBody::ThreadShared(_) | EventBody::ThreadUnshared(_)
            )
        })
        .cloned()
        .collect();
    let without = project(&plain);
    let kinds = |frames: &[Vec<Frame>]| -> Vec<String> {
        support::flatten(frames).iter().map(support::line).collect()
    };
    assert_eq!(kinds(&frames), kinds(&without));
}

#[test]
fn an_error_before_a_share_still_explains_the_state_that_follows() {
    // the projector remembers that the previous event was an error, and lets the `thread_state` that
    // may follow decide; a sharing event between them must not make it forget
    let error = ev(
        2,
        Actor::system(),
        EventBody::Error(ErrorData {
            message: "the agent could not be reached".into(),
            retryable: false,
        }),
    );
    let failed = ev(
        4,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData {
            state: ThreadState::Failed,
        }),
    );
    let first = ev(
        1,
        owner(),
        EventBody::UserMessage(UserMessageData::new("go")),
    );
    let with = project(&[first.clone(), error.clone(), shared(3), failed.clone()]);
    let without = project(&[first, error, failed.clone()]);
    let said = |frames: &[Vec<Frame>]| -> Vec<String> {
        support::flatten(frames).iter().map(support::line).collect()
    };
    assert_eq!(said(&with), said(&without));
}
