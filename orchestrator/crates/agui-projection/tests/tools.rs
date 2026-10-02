//! The MCP servers attached to a thread (`tools_attached`, `tools_detached`, ADR 0024,
//! `docs/api/agui.md`, "Tools"), row by row, on hand-written logs: the set is part of every
//! `STATE_SNAPSHOT` (`thread.tools`) and a change is a `vymalo.tools` card, inside a run or, when
//! nothing is going on, as a run of its own.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, Event, EventBody, ThreadState, ThreadStateData,
    Timestamp, ToolsData, UserId, UserMessageData,
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

fn data(servers: &[&str]) -> ToolsData {
    ToolsData {
        servers: servers.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn attached(seq: i64, servers: &[&str]) -> Event {
    ev(
        seq,
        Actor::user(&alice()),
        EventBody::ToolsAttached(data(servers)),
    )
}

fn detached(seq: i64, servers: &[&str]) -> Event {
    ev(
        seq,
        Actor::user(&alice()),
        EventBody::ToolsDetached(data(servers)),
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

/// `thread.tools` of each `STATE_SNAPSHOT` (`Null` for one that says none).
fn snapshot_tools(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::StateSnapshot(s) => Some(s.snapshot["thread"]["tools"].clone()),
            _ => None,
        })
        .collect()
}

fn cards(frames: &[Frame]) -> Vec<(String, Value)> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(a) if a.activity_type == "vymalo.tools" => {
                Some((a.message_id.to_string(), json!(a.content)))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn inside_a_run_it_is_the_new_snapshot_then_a_card() {
    let events = [
        user(1, "go"),
        attached(2, &["docs", "websearch"]),
        status(3, AgentStatus::Working),
    ];
    let frames = project(&events);
    let got = lines(&frames[1]);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[0].starts_with("STATE_SNAPSHOT queued"), "{got:?}");
    assert!(got[1].contains("evt-2 vymalo.tools"), "{got:?}");
    assert!(
        got[1].ends_with("id:2"),
        "the card is the resume point: {got:?}"
    );
    assert_eq!(snapshot_tools(&frames[1]), [json!(["docs", "websearch"])]);
    assert_eq!(snapshot_tools(&frames[0]), [Value::Null], "before: none");
    let card = cards(&frames[1]);
    assert_eq!(card.len(), 1);
    assert_eq!(card[0].0, "evt-2");
    assert_eq!(card[0].1["attached"], json!(["docs", "websearch"]));
    assert!(card[0].1.get("detached").is_none());
    assert!(card[0].1["at"].is_string(), "like every vymalo.* card");
    // and what comes after says the set too
    assert_eq!(snapshot_tools(&frames[2]), [json!(["docs", "websearch"])]);
}

#[test]
fn a_detach_is_a_card_with_the_ids_that_went_and_the_set_that_is_left() {
    let events = [
        user(1, "go"),
        attached(2, &["docs", "websearch"]),
        detached(3, &["docs"]),
        detached(4, &["websearch"]),
        status(5, AgentStatus::Working),
    ];
    let frames = project(&events);
    assert_eq!(snapshot_tools(&frames[2]), [json!(["websearch"])]);
    let card = cards(&frames[2]);
    assert_eq!(card[0].1["detached"], json!(["docs"]));
    assert!(card[0].1.get("attached").is_none());
    // the last one empties the set, which a snapshot says by leaving the member out
    assert_eq!(snapshot_tools(&frames[3]), [Value::Null]);
    assert_eq!(snapshot_tools(&frames[4]), [Value::Null]);
}

#[test]
fn on_a_finished_thread_it_is_a_run_of_its_own_with_the_snapshot_and_the_card() {
    let events = [
        user(1, "echo"),
        status(2, AgentStatus::Working),
        status(3, AgentStatus::Completed),
        thread(4, ThreadState::Done),
        attached(5, &["websearch"]),
    ];
    let frames = project(&events);
    let got = lines(&frames[4]);
    assert_eq!(got.first().map(String::as_str), Some("RUN_STARTED run-5"));
    assert!(
        got.iter().any(|l| l.starts_with("STATE_SNAPSHOT done")),
        "{got:?}"
    );
    assert!(got.iter().any(|l| l.contains("vymalo.tools")), "{got:?}");
    assert!(
        got.last()
            .unwrap()
            .starts_with("RUN_FINISHED run-5 success"),
        "{got:?}"
    );
    assert_eq!(snapshot_tools(&frames[4]), [json!(["websearch"])]);
    assert_eq!(cards(&frames[4])[0].0, "evt-5");
}

#[test]
fn on_a_cancelled_or_failed_thread_the_run_ends_as_the_thread_ended() {
    for (last, state, ends) in [
        (AgentStatus::Canceled, ThreadState::Cancelled, "cancelled"),
        (AgentStatus::Failed, ThreadState::Failed, "RUN_ERROR"),
    ] {
        let events = [
            user(1, "x"),
            status(2, AgentStatus::Working),
            status(3, last),
            thread(4, state),
            attached(5, &["websearch"]),
        ];
        let frames = project(&events);
        let got = lines(&frames[4]);
        assert!(got.last().unwrap().contains(ends), "{state:?}: {got:?}");
    }
}

#[test]
fn the_creation_commit_says_the_servers_inside_the_run_the_message_opened() {
    // what `create_thread_as` writes: the message, then the servers
    let events = [user(1, "go"), attached(2, &["websearch"])];
    let frames = project(&events);
    let got = lines(&frames[0]);
    assert_eq!(got.first().map(String::as_str), Some("RUN_STARTED run-1"));
    assert_eq!(
        lines(&frames[1]).first().map(String::as_str),
        Some("STATE_SNAPSHOT queued")
    );
}

#[test]
fn a_new_job_keeps_the_servers() {
    let events = [
        user(1, "one"),
        attached(2, &["docs"]),
        status(3, AgentStatus::Working),
        status(4, AgentStatus::Completed),
        thread(5, ThreadState::Done),
        user(6, "two"),
    ];
    let frames = project(&events);
    assert_eq!(
        snapshot_tools(&frames[5]),
        [json!(["docs"])],
        "the second job's first snapshot still says them"
    );
}

#[test]
fn a_viewer_that_resumes_after_the_change_is_told_the_set_in_the_preamble() {
    let events = [
        user(1, "go"),
        attached(2, &["docs"]),
        status(3, AgentStatus::Working),
    ];
    let mut connect =
        orch_agui_projection::Connect::new(meta(), 3, 3, orch_agui_projection::Follow::Forever);
    let mut said = Vec::new();
    for event in &events {
        said.extend(connect.feed(event));
    }
    assert_eq!(
        snapshot_tools(&said),
        [json!(["docs"])],
        "the preamble's snapshot"
    );
    assert!(
        cards(&said).is_empty(),
        "a card the client already holds is not said again"
    );
}

#[test]
fn only_ids_are_ever_said() {
    let events = [user(1, "go"), attached(2, &["docs"])];
    let frames = project(&events);
    let text = serde_json::to_string(
        &support::flatten(&frames)
            .iter()
            .map(|f| serde_json::to_value(&f.event).unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!text.contains("http"), "{text}");
}
