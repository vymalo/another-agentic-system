//! A fork (`thread_forked`, ADR 0029) in the projection, until its frames exist: the event moves the
//! state to `done` (a fork is a finished job) and the title to the parent's, and says nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentTarget, Event, EventBody, ForkKind, ForkSource, ThreadForkedData,
    Timestamp, UserId, UserMessageData,
};
use support::log::{meta, thread_id};

fn ev(seq: i64, actor: Actor, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    }
}

#[test]
fn a_fork_says_nothing_and_the_next_message_is_a_new_job_with_the_parents_title() {
    let alice = UserId::new("alice@example.com");
    let forked = ev(
        1,
        Actor::user(&alice),
        EventBody::ThreadForked(ThreadForkedData {
            from: ForkSource {
                thread_id: thread_id(),
                seq: 0,
            },
            kind: ForkKind::Fork,
            title: "Fix the redirect loop".to_owned(),
            target: AgentTarget {
                agent_id: AgentId::new("plain"),
                release: None,
            },
        }),
    );
    let message = ev(
        2,
        Actor::user(&alice),
        EventBody::UserMessage(UserMessageData::new("go on")),
    );
    let mut projector = Projector::new(meta());
    assert!(projector.apply(&forked, Audience::Viewer).is_empty());
    let frames: Vec<Frame> = projector.apply(&message, Audience::Viewer);
    let titles: Vec<String> = frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::StateSnapshot(s) => {
                s.snapshot["thread"]["title"].as_str().map(str::to_owned)
            }
            _ => None,
        })
        .collect();
    assert_eq!(titles, ["Fix the redirect loop"]);
}
