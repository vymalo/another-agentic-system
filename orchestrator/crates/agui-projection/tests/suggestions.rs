//! The suggestions of a job (`thread_suggested`, `docs/api/agui.md`, "Suggestions"), row by row,
//! on hand-written logs: a job-scoped card of its own (`vymalo.suggestions`), never the thread's
//! state and never a message.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_agui_proto::{ActivitySnapshotEvent, Event as Agui};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, Event, EventBody, ThreadState, ThreadStateData,
    ThreadSuggestedData, Timestamp, UserId, UserMessageData,
};
use serde_json::Value;
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

/// The model offers `suggestions` under the final answer of `job`.
fn suggested(seq: i64, job: u32, suggestions: &[&str]) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::ThreadSuggested(ThreadSuggestedData {
            job,
            suggestions: suggestions.iter().map(|s| (*s).to_owned()).collect(),
        }),
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

/// Every `vymalo.suggestions` card: its message id and content.
fn cards(frames: &[Frame]) -> Vec<(String, Value)> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            Agui::ActivitySnapshot(e) if e.activity_type == "vymalo.suggestions" => Some((
                e.message_id.as_str().to_owned(),
                Value::Object(e.content.clone()),
            )),
            _ => None,
        })
        .collect()
}

fn card(frames: &[Frame]) -> &ActivitySnapshotEvent {
    frames
        .iter()
        .find_map(|f| match &f.event {
            Agui::ActivitySnapshot(e) if e.activity_type == "vymalo.suggestions" => Some(e),
            _ => None,
        })
        .unwrap()
}

#[test]
fn the_suggestions_of_a_finished_job_are_a_card_in_a_run_of_its_own() {
    let events = [
        user(1, "fix the loop"),
        status(2, AgentStatus::Working),
        status(3, AgentStatus::Completed),
        thread(4, ThreadState::Done),
        suggested(5, 1, &["Run the tests", "Open a pull request"]),
    ];
    let frames = project(&events);
    assert_eq!(
        lines(&frames[4]),
        [
            "RUN_STARTED run-5",
            "ACTIVITY_SNAPSHOT suggestions-1 vymalo.suggestions {\"job\":1,\"suggestions\":[\"Run the tests\",\"Open a pull request\"]}",
            "STATE_SNAPSHOT done",
            "RUN_FINISHED run-5 success  id:5"
        ],
        "a run of its own holds the card and closes at once"
    );
    let got = card(&frames[4]);
    assert_eq!(got.message_id.as_str(), "suggestions-1");
    assert_eq!(got.replace, None, "one event per job: nothing to replace");
    assert_eq!(got.content["job"], Value::from(1));
    assert_eq!(
        got.content["suggestions"],
        serde_json::json!(["Run the tests", "Open a pull request"])
    );
    assert!(got.content.contains_key("at"), "every card says when");
}

#[test]
fn inside_a_run_the_card_is_all_that_is_said() {
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Working),
        suggested(3, 1, &["Try again"]),
    ];
    let frames = project(&events);
    assert_eq!(
        lines(&frames[2]),
        [
            "ACTIVITY_SNAPSHOT suggestions-1 vymalo.suggestions {\"job\":1,\"suggestions\":[\"Try again\"]}  id:3"
        ],
        "no run is opened and no snapshot is said"
    );
}

#[test]
fn the_suggestions_are_never_the_threads_state_nor_a_message() {
    let events = [
        user(1, "fix the loop"),
        status(2, AgentStatus::Completed),
        thread(3, ThreadState::Done),
        suggested(4, 1, &["Run the tests"]),
        // a later snapshot, for another reason, still says nothing about suggestions
        status(5, AgentStatus::Working),
    ];
    let frames = project(&events);
    for entry in &frames {
        for frame in entry {
            if let Agui::StateSnapshot(s) = &frame.event {
                assert!(
                    s.snapshot["thread"].get("suggestions").is_none(),
                    "suggestions are job-scoped, not the thread's: {}",
                    s.snapshot
                );
            }
        }
    }
    // The event's own frames are the run and the card: no message is said for a suggestion.
    assert!(
        frames[3].iter().all(|f| !matches!(
            &f.event,
            Agui::TextMessageStart(_) | Agui::TextMessageContent(_)
        )),
        "{:?}",
        lines(&frames[3])
    );
}

#[test]
fn each_job_has_its_own_card() {
    let events = [
        user(1, "first"),
        status(2, AgentStatus::Completed),
        thread(3, ThreadState::Done),
        suggested(4, 1, &["Ask about the second"]),
        user(5, "second"),
        thread(6, ThreadState::Done),
        suggested(7, 2, &["Run the second test"]),
    ];
    let frames = support::flatten(&project(&events));
    assert_eq!(
        cards(&frames),
        [
            (
                "suggestions-1".to_owned(),
                serde_json::json!({
                    "job": 1,
                    "suggestions": ["Ask about the second"],
                    "at": "2027-01-15T08:00:04Z",
                })
            ),
            (
                "suggestions-2".to_owned(),
                serde_json::json!({
                    "job": 2,
                    "suggestions": ["Run the second test"],
                    "at": "2027-01-15T08:00:07Z",
                })
            ),
        ]
    );
}
