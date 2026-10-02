//! The mapping table of `docs/api/agui.md`, row by row, on hand-written logs.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_agui_projection::{Audience, Frame, Projector, ThreadMeta};
use orch_core::{
    Actor, AgentId, AgentMessageData, AgentStatus, AgentStatusData, AnswerVia, ArtifactData,
    ErrorData, Event, EventBody, MessagePurpose, ThreadState, ThreadStateData, Timestamp, UserId,
    UserMessageData,
};
use support::log::{THREAD, meta, thread_id};
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

fn alice() -> Actor {
    Actor::user(&UserId::new("alice@example.com"))
}

fn plain() -> Actor {
    Actor::agent(&AgentId::new("plain"), None)
}

fn user(seq: i64, text: &str) -> Event {
    ev(
        seq,
        alice(),
        EventBody::UserMessage(UserMessageData::new(text)),
    )
}

fn user_ids(seq: i64, text: &str, message_id: &str, run_id: &str) -> Event {
    ev(
        seq,
        alice(),
        EventBody::UserMessage(UserMessageData {
            text: text.to_owned(),
            message_id: Some(message_id.to_owned()),
            run_id: Some(run_id.to_owned()),
            origin: orch_core::Origin::Agui,
        }),
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

fn error(seq: i64, message: &str, retryable: bool) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::Error(ErrorData {
            message: message.to_owned(),
            retryable,
        }),
    )
}

fn say(seq: i64, id: &str, text: &str, is_final: bool) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentMessage(AgentMessageData {
            text: text.to_owned(),
            message_id: id.to_owned(),
            is_final,
            purpose: None,
            via: None,
        }),
    )
}

fn artifact(seq: i64) -> Event {
    ev(
        seq,
        plain(),
        EventBody::Artifact(ArtifactData {
            name: "result".to_owned(),
            mime_type: None,
            uri: Some("https://example.com/pr/1".to_owned()),
            text: None,
            file: None,
        }),
    )
}

/// Projects `events` for a viewer, one entry per event, after checking the stream.
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

/// A log that reaches `blocked` through an agent asking a question.
fn asked() -> Vec<Event> {
    vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::InputRequired, Some("Which branch?")),
        thread(4, ThreadState::Blocked),
    ]
}

#[test]
fn a_thread_that_asks_ends_its_run_with_an_interrupt_and_suspends_the_invocation() {
    let got = all_lines(&asked());
    assert_eq!(
        got[5..],
        [
            "SUBAGENT_STARTED sub-2 plain",
            "ACTIVITY_SNAPSHOT evt-2 vymalo.status {\"status\":\"working\"} @sub-2",
            "STATE_SNAPSHOT working  id:2",
            // The question is the agent's words: an assistant message, named after the agent.
            "TEXT_MESSAGE_START st-3 assistant @sub-2",
            "TEXT_MESSAGE_CONTENT st-3 \"Which branch?\"",
            "TEXT_MESSAGE_END st-3",
            "ACTIVITY_SNAPSHOT evt-3 vymalo.status {\"status\":\"input_required\"} @sub-2",
            "SUBAGENT_FINISHED sub-2 suspended[int-3]  id:3",
            "STATE_SNAPSHOT blocked",
            "RUN_FINISHED run-1 interrupt[int-3:input_required @sub-2]  id:4",
        ]
    );
}

#[test]
fn the_interrupt_carries_the_prompt_and_the_shape_of_the_answer() {
    let frames = support::flatten(&project(&asked()));
    let finished = frames
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::RunFinished(e) => Some(e.clone()),
            _ => None,
        })
        .unwrap();
    let Some(orch_agui_proto::RunFinishedOutcome::Interrupt { interrupts }) = finished.outcome
    else {
        panic!("not an interrupt outcome");
    };
    assert_eq!(interrupts.len(), 1);
    let interrupt = &interrupts[0];
    assert_eq!(interrupt.message.as_deref(), Some("Which branch?"));
    assert_eq!(
        serde_json::to_value(interrupt.response_schema.as_ref().unwrap()).unwrap(),
        serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": {"text": {"type": "string"}}
        })
    );
}

#[test]
fn the_answer_opens_a_new_run_and_the_suspended_invocation_reappears_under_its_id() {
    let mut events = asked();
    events.push(user(5, "main"));
    events.push(status(6, AgentStatus::Working, None));
    events.push(status(7, AgentStatus::Completed, None));
    events.push(thread(8, ThreadState::Done));
    let got = all_lines(&events);
    let second: Vec<_> = got
        .iter()
        .skip_while(|l| !l.starts_with("RUN_STARTED run-5"))
        .collect();
    assert_eq!(second[0], "RUN_STARTED run-5");
    assert_eq!(second[1], "STATE_SNAPSHOT queued");
    assert!(
        second
            .iter()
            .any(|l| l.as_str() == "SUBAGENT_STARTED sub-2 plain")
    );
    assert_eq!(
        second.last().unwrap().as_str(),
        "RUN_FINISHED run-5 success  id:8"
    );
}

#[test]
fn auth_required_is_an_interrupt_of_its_own_reason() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::AuthRequired, Some("sign in to GitHub")),
        thread(3, ThreadState::Blocked),
    ];
    let got = all_lines(&events);
    assert!(got.contains(&"TEXT_MESSAGE_CONTENT st-2 \"sign in to GitHub\"".to_owned()));
    assert!(got.contains(
        &"ACTIVITY_SNAPSHOT evt-2 vymalo.status {\"status\":\"auth_required\"} @sub-2".to_owned()
    ));
    assert_eq!(
        got.last().unwrap(),
        "RUN_FINISHED run-1 interrupt[int-2:auth_required @sub-2]  id:3"
    );
}

#[test]
fn a_failed_agent_ends_the_run_in_error_with_the_problem() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::Failed, Some("boom")),
        thread(4, ThreadState::Failed),
    ];
    let frames = support::flatten(&project(&events));
    let got = lines(&frames);
    assert_eq!(
        got[got.len() - 3..],
        [
            "SUBAGENT_ERROR sub-2 agent_failed \"boom\"  id:3",
            "STATE_SNAPSHOT failed",
            "RUN_ERROR agent_failed \"boom\"  id:4",
        ]
    );
    let orch_agui_proto::Event::RunError(e) = &frames.last().unwrap().event else {
        panic!("last frame is not RUN_ERROR");
    };
    assert_eq!(
        serde_json::to_value(e.base.metadata.as_ref().unwrap()).unwrap(),
        serde_json::json!({"vymalo.problem": {"type": "about:blank", "title": "Agent failed", "detail": "boom"}})
    );
}

#[test]
fn a_failure_without_a_detail_still_says_something() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Failed, None),
        thread(3, ThreadState::Failed),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got.last().unwrap(),
        "RUN_ERROR agent_failed \"the agent failed\"  id:3"
    );
}

#[test]
fn a_cancelled_agent_ends_the_run_as_cancelled_and_says_so_on_the_invocation() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::Canceled, Some("canceled")),
        thread(4, ThreadState::Cancelled),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got[got.len() - 3..],
        [
            "SUBAGENT_FINISHED sub-2 success result={\"status\":\"canceled\"}  id:3",
            "STATE_SNAPSHOT cancelled",
            "RUN_FINISHED run-1 cancelled  id:4",
        ]
    );
}

#[test]
fn a_thread_cancelled_before_the_agent_started_has_no_invocation() {
    let events = vec![user(1, "go"), thread(2, ThreadState::Cancelled)];
    let got = all_lines(&events);
    assert!(!got.iter().any(|l| l.starts_with("SUBAGENT")), "{got:?}");
    assert_eq!(
        got[5..],
        [
            "STATE_SNAPSHOT cancelled",
            "RUN_FINISHED run-1 cancelled  id:2"
        ]
    );
}

#[test]
fn a_retryable_delivery_failure_ends_the_run_in_error_and_the_thread_stays_open() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        error(3, "agent unreachable", true),
        thread(4, ThreadState::Blocked),
        // The next input is an ordinary new run, not a resume.
        user(5, "try again"),
        status(6, AgentStatus::Working, None),
    ];
    let got = all_lines(&events);
    let at = got
        .iter()
        .position(|l| l.starts_with("ACTIVITY_SNAPSHOT evt-3 vymalo.error"))
        .unwrap();
    assert_eq!(
        got[at..at + 5],
        [
            "ACTIVITY_SNAPSHOT evt-3 vymalo.error {\"message\":\"agent unreachable\",\"retryable\":true}  id:3",
            "SUBAGENT_ERROR sub-2 delivery_failed \"agent unreachable\"",
            "STATE_SNAPSHOT blocked",
            "RUN_ERROR delivery_failed \"agent unreachable\"  id:4",
            "RUN_STARTED run-5",
        ]
    );
    // The invocation ended in error, so the retry is a new invocation, not a continuation.
    assert!(
        got.contains(&"SUBAGENT_STARTED sub-6 plain".to_owned()),
        "{got:?}"
    );
}

// ---- the UI's catalog (ADR 0023) ----------------------------------------------------------------

fn ui_catalog(seq: i64, version: u32) -> Event {
    ev(
        seq,
        alice(),
        EventBody::UiCatalog(support::log::catalog(match version {
            1 => 0,
            2 => 1,
            _ => 3,
        })),
    )
}

#[test]
fn a_ui_catalog_gives_no_frames_and_no_resume_point_and_the_run_opens_with_the_message() {
    let mut projector = Projector::new(meta());
    // first in its commit, ahead of the message that opens the run
    assert_eq!(projector.apply(&ui_catalog(1, 1), Audience::Viewer), vec![]);
    assert!(!projector.run_open());
    let frames = projector.apply(&user(2, "go"), Audience::Viewer);
    assert_eq!(lines(&frames)[0], "RUN_STARTED run-2");
    // in the middle of a run: still nothing, and the run goes on
    assert_eq!(projector.apply(&ui_catalog(3, 2), Audience::Viewer), vec![]);
    assert!(projector.run_open());
    let more = projector.apply(&status(4, AgentStatus::Working, None), Audience::Viewer);
    assert!(!more.is_empty());
}

#[test]
fn a_ui_catalog_between_an_error_and_the_state_it_explains_changes_nothing_they_say() {
    let without = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        error(3, "agent unreachable", true),
        thread(4, ThreadState::Blocked),
    ];
    let with = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        error(3, "agent unreachable", true),
        ui_catalog(4, 1),
        thread(5, ThreadState::Blocked),
    ];
    let (a, b) = (project(&without), project(&with));
    assert_eq!(b[3], vec![], "the catalog says nothing");
    // the closing frames are the same, but for the resume point (the event's number) and the
    // snapshot, which names the catalog the thread was shown (ADR 0023)
    let values = |frames: &Vec<Frame>| -> Vec<serde_json::Value> {
        frames
            .iter()
            .map(|f| {
                let mut value = serde_json::to_value(&f.event).unwrap();
                if let Some(thread) = value
                    .pointer_mut("/snapshot/thread")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    thread.remove("uiCatalog");
                }
                value
            })
            .collect()
    };
    assert_eq!(values(&a[3]), values(&b[4]));
    let snapshot = b[4]
        .iter()
        .map(|f| serde_json::to_value(&f.event).unwrap())
        .find(|v| v["type"] == "STATE_SNAPSHOT")
        .expect("the thread state says where the thread stands");
    assert_eq!(snapshot["snapshot"]["thread"]["uiCatalog"]["version"], 1);
    assert!(
        lines(&b[4])
            .iter()
            .any(|l| l.starts_with("RUN_ERROR delivery_failed")),
        "{:?}",
        lines(&b[4])
    );
}

#[test]
fn a_permanent_delivery_failure_fails_the_thread() {
    let events = vec![
        user(1, "go"),
        error(2, "no such agent", false),
        thread(3, ThreadState::Failed),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got[5..],
        [
            "ACTIVITY_SNAPSHOT evt-2 vymalo.error {\"message\":\"no such agent\",\"retryable\":false}  id:2",
            "STATE_SNAPSHOT failed",
            "RUN_ERROR delivery_failed \"no such agent\"  id:3",
        ]
    );
}

#[test]
fn a_late_delivery_failure_after_the_thread_finished_is_a_run_of_its_own() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Completed, None),
        thread(3, ThreadState::Done),
        error(4, "late failure", false),
    ];
    let got = all_lines(&events);
    let at = got.iter().position(|l| l == "RUN_STARTED run-4").unwrap();
    assert_eq!(
        got[at..],
        [
            "RUN_STARTED run-4",
            "STATE_SNAPSHOT done",
            "ACTIVITY_SNAPSHOT evt-4 vymalo.error {\"message\":\"late failure\",\"retryable\":false}",
            "STATE_SNAPSHOT done",
            "RUN_ERROR delivery_failed \"late failure\"  id:4",
        ]
    );
}

#[test]
fn a_refused_cancel_while_working_is_an_error_line_and_the_run_goes_on() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        error(3, "the agent refused to cancel", false),
        status(4, AgentStatus::Completed, None),
        thread(5, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got.iter().filter(|l| l.starts_with("RUN_STARTED")).count(),
        1
    );
    assert!(
        got.iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT evt-3 vymalo.error"))
    );
    assert_eq!(got.last().unwrap(), "RUN_FINISHED run-1 success  id:5");
    // The error did not touch the invocation.
    assert!(
        !got.iter().any(|l| l.starts_with("SUBAGENT_ERROR")),
        "{got:?}"
    );
}

#[test]
fn a_refused_cancel_while_blocked_re_raises_the_interrupt_in_a_run_of_its_own() {
    let mut events = asked();
    events.push(error(5, "cancel refused", true));
    let got = all_lines(&events);
    let at = got.iter().position(|l| l == "RUN_STARTED run-5").unwrap();
    assert_eq!(
        got[at..],
        [
            "RUN_STARTED run-5",
            "STATE_SNAPSHOT blocked",
            "ACTIVITY_SNAPSHOT evt-5 vymalo.error {\"message\":\"cancel refused\",\"retryable\":true}",
            "STATE_SNAPSHOT blocked",
            "RUN_FINISHED run-5 interrupt[int-3:input_required @sub-2]  id:5",
        ]
    );
}

#[test]
fn a_wait_repeated_while_blocked_is_a_run_of_its_own_with_a_new_interrupt() {
    let mut events = asked();
    events.push(status(5, AgentStatus::InputRequired, Some("Which remote?")));
    let got = all_lines(&events);
    let at = got.iter().position(|l| l == "RUN_STARTED run-5").unwrap();
    assert_eq!(
        got[at..],
        [
            "RUN_STARTED run-5",
            "STATE_SNAPSHOT blocked",
            // The invocation that suspended reappears under its own id.
            "SUBAGENT_STARTED sub-2 plain",
            "TEXT_MESSAGE_START st-5 assistant @sub-2",
            "TEXT_MESSAGE_CONTENT st-5 \"Which remote?\"",
            "TEXT_MESSAGE_END st-5",
            "ACTIVITY_SNAPSHOT evt-5 vymalo.status {\"status\":\"input_required\"} @sub-2",
            "SUBAGENT_FINISHED sub-2 suspended[int-5]",
            "STATE_SNAPSHOT blocked",
            "RUN_FINISHED run-5 interrupt[int-5:input_required @sub-2]  id:5",
        ]
    );
}

#[test]
fn an_artifact_that_arrives_while_blocked_is_shown_in_a_run_that_ends_blocked() {
    let mut events = asked();
    events.push(artifact(5));
    let got = all_lines(&events);
    let at = got.iter().position(|l| l == "RUN_STARTED run-5").unwrap();
    assert!(got[at + 3].starts_with("ACTIVITY_SNAPSHOT evt-5 vymalo.artifact"));
    assert_eq!(
        got.last().unwrap(),
        "RUN_FINISHED run-5 interrupt[int-3:input_required @sub-2]  id:5"
    );
}

#[test]
fn an_agent_that_starts_working_again_while_blocked_reopens_a_run_that_stays_open() {
    let mut events = asked();
    events.push(status(5, AgentStatus::Working, None));
    let mut projector = Projector::new(meta());
    for e in &events {
        projector.apply(e, Audience::Viewer);
    }
    assert!(projector.run_open());
    assert_eq!(projector.thread_state(), ThreadState::Working);
    let got = all_lines(&events);
    let at = got.iter().position(|l| l == "RUN_STARTED run-5").unwrap();
    assert_eq!(got[at + 1], "STATE_SNAPSHOT working");
}

#[test]
fn a_follow_up_during_a_run_is_inside_the_run() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        user(3, "also this"),
        status(4, AgentStatus::Completed, None),
        thread(5, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got.iter().filter(|l| l.starts_with("RUN_STARTED")).count(),
        1
    );
    assert!(got.contains(&"TEXT_MESSAGE_START evt-3 user".to_owned()));
}

#[test]
fn an_agent_message_streams_inside_its_invocation_and_ends_with_a_resume_point() {
    let events = vec![
        user(1, "go"),
        say(2, "m-1", "Plan: add a test", true),
        status(3, AgentStatus::Completed, None),
        thread(4, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got[5..9],
        [
            "SUBAGENT_STARTED sub-2 plain",
            "TEXT_MESSAGE_START m-1 assistant @sub-2",
            "TEXT_MESSAGE_CONTENT m-1 \"Plan: add a test\"",
            "TEXT_MESSAGE_END m-1  id:2",
        ]
    );
}

#[test]
fn partials_stream_as_deltas_and_no_resume_point_splits_the_message() {
    let events = vec![
        user(1, "go"),
        say(2, "m-1", "Plan", false),
        say(3, "m-1", "Plan: add", false),
        say(4, "m-1", "Plan: add a test", true),
        status(5, AgentStatus::Completed, None),
        thread(6, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got[5..12],
        [
            "SUBAGENT_STARTED sub-2 plain",
            "TEXT_MESSAGE_START m-1 assistant @sub-2",
            "TEXT_MESSAGE_CONTENT m-1 \"Plan\"",
            "TEXT_MESSAGE_CONTENT m-1 \": add\"",
            "TEXT_MESSAGE_CONTENT m-1 \" a test\"",
            "TEXT_MESSAGE_END m-1  id:4",
            "ACTIVITY_SNAPSHOT evt-5 vymalo.status {\"status\":\"completed\"} @sub-2",
        ]
    );
}

#[test]
fn a_message_left_open_is_closed_before_its_invocation_and_run_end() {
    let events = vec![
        user(1, "go"),
        say(2, "m-1", "Plan", false),
        status(3, AgentStatus::Canceled, None),
        thread(4, ThreadState::Cancelled),
    ];
    let got = all_lines(&events);
    let at = got
        .iter()
        .position(|l| l.starts_with("TEXT_MESSAGE_END m-1"))
        .unwrap();
    assert!(
        got[at + 1].starts_with("SUBAGENT_FINISHED sub-2"),
        "{got:?}"
    );
    assert!(
        got.last()
            .unwrap()
            .starts_with("RUN_FINISHED run-1 cancelled")
    );
}

#[test]
fn a_partial_that_does_not_extend_the_text_starts_a_new_message_under_a_derived_id() {
    let events = vec![
        user(1, "go"),
        say(2, "m-1", "Plan A", false),
        say(3, "m-1", "Something else", true),
        status(4, AgentStatus::Completed, None),
        thread(5, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got[5..12],
        [
            "SUBAGENT_STARTED sub-2 plain",
            "TEXT_MESSAGE_START m-1 assistant @sub-2",
            "TEXT_MESSAGE_CONTENT m-1 \"Plan A\"",
            "TEXT_MESSAGE_END m-1",
            "TEXT_MESSAGE_START m-1~3 assistant @sub-2",
            "TEXT_MESSAGE_CONTENT m-1~3 \"Something else\"",
            "TEXT_MESSAGE_END m-1~3  id:3",
        ]
    );
}

#[test]
fn the_same_final_message_twice_is_said_once() {
    let events = vec![
        user(1, "go"),
        say(2, "m-1", "hello", true),
        say(3, "m-1", "hello", true),
        status(4, AgentStatus::Completed, None),
        thread(5, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got.iter()
            .filter(|l| l.starts_with("TEXT_MESSAGE_START m-1"))
            .count(),
        1
    );
}

#[test]
fn the_revision_of_a_release_is_echoed_on_every_agent_attributed_frame() {
    let coder = Actor::agent(&AgentId::new("coder"), Some("coder-r51".to_owned()));
    let events = vec![
        user(1, "go"),
        ev(
            2,
            coder,
            EventBody::AgentStatus(AgentStatusData {
                status: AgentStatus::Working,
                detail: None,
            }),
        ),
    ];
    let frames = support::flatten(&project(&events));
    for frame in &frames {
        let json = serde_json::to_value(&frame.event).unwrap();
        let tag = json["type"].as_str().unwrap();
        if matches!(tag, "SUBAGENT_STARTED" | "ACTIVITY_SNAPSHOT") && json["name"] != "alice" {
            let actor = &json["metadata"]["vymalo.actor"];
            assert_eq!(actor["type"], "agent", "{json}");
            assert_eq!(actor["name"], "coder", "{json}");
            assert_eq!(actor["revision"], "coder-r51", "{json}");
        }
    }
}

#[test]
fn the_user_message_names_its_author_in_the_actor_metadata() {
    let frames = support::flatten(&project(&[user(1, "go")]));
    let start = frames
        .iter()
        .find(|f| matches!(f.event, orch_agui_proto::Event::TextMessageStart(_)))
        .unwrap();
    let json = serde_json::to_value(&start.event).unwrap();
    assert_eq!(
        json["metadata"],
        serde_json::json!({"vymalo.actor": {"type": "user", "name": "alice@example.com"}})
    );
}

#[test]
fn ids_the_surface_recorded_name_the_message_and_the_run() {
    let events = vec![
        user_ids(1, "go", "client-msg-1", "client-run-1"),
        status(2, AgentStatus::Completed, None),
        thread(3, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert_eq!(got[0], "RUN_STARTED client-run-1");
    assert_eq!(got[2], "TEXT_MESSAGE_START client-msg-1 user");
    assert_eq!(
        got.last().unwrap(),
        "RUN_FINISHED client-run-1 success  id:3"
    );
}

#[test]
fn every_frame_carries_the_thread_and_the_protocol_version() {
    let frames = support::flatten(&project(&asked()));
    let started = frames
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::RunStarted(e) => Some(e.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(started.thread_id.as_str(), THREAD);
    assert_eq!(started.protocol_version.as_deref(), Some("1.0"));
    assert!(started.input.is_none(), "no input echo");
}

#[test]
fn the_state_snapshot_carries_the_title_and_the_target() {
    let mut m = meta();
    m.target.release = Some("staging".to_owned());
    let mut projector = Projector::new(ThreadMeta {
        title: "Fix the flaky test".to_owned(),
        ..m
    });
    let frames = projector.apply(&user(1, "go"), Audience::Viewer);
    let orch_agui_proto::Event::StateSnapshot(snapshot) = &frames[1].event else {
        panic!("second frame is not a snapshot");
    };
    assert_eq!(
        snapshot.snapshot,
        serde_json::json!({"thread": {
            "state": "queued",
            "title": "Fix the flaky test",
            "target": {"agentId": "plain", "release": "staging"}
        }})
    );
}

// ---- audiences ---------------------------------------------------------------------------

#[test]
fn a_requester_does_not_get_back_the_message_it_sent_but_a_viewer_does() {
    let events = [
        user_ids(1, "go", "client-msg-1", "client-run-1"),
        status(2, AgentStatus::Completed, None),
        thread(3, ThreadState::Done),
    ];
    let held: BTreeSet<String> = ["client-msg-1".to_owned()].into();
    let mut requester = Projector::new(meta());
    let mut viewer = Projector::new(meta());
    let req: Vec<Frame> = events
        .iter()
        .flat_map(|e| {
            requester.apply(
                e,
                Audience::Requester {
                    held_message_ids: &held,
                },
            )
        })
        .collect();
    let view: Vec<Frame> = events
        .iter()
        .flat_map(|e| viewer.apply(e, Audience::Viewer))
        .collect();
    assert!(
        lines(&req).iter().all(|l| !l.starts_with("TEXT_MESSAGE")),
        "{:?}",
        lines(&req)
    );
    assert!(
        lines(&view)
            .iter()
            .any(|l| l.starts_with("TEXT_MESSAGE_START client-msg-1"))
    );
    // Everything else is the same story, in the same order.
    let without_triad: Vec<String> = lines(&view)
        .into_iter()
        .filter(|l| !l.contains("client-msg-1"))
        .collect();
    let mut req_lines = lines(&req);
    // The resume point of the log event moves off the skipped frames: same event, same id.
    let strip = |v: &mut Vec<String>| {
        v.iter_mut()
            .for_each(|l| *l = l.split("  id:").next().unwrap().to_owned())
    };
    let mut without_triad = without_triad;
    strip(&mut without_triad);
    strip(&mut req_lines);
    assert_eq!(req_lines, without_triad);
}

#[test]
fn a_requester_still_gets_a_message_it_does_not_hold() {
    // An answer sent through `resume` is not one of the request's messages.
    let held: BTreeSet<String> = ["something-else".to_owned()].into();
    let mut projector = Projector::new(meta());
    let frames = projector.apply(
        &user(1, "main"),
        Audience::Requester {
            held_message_ids: &held,
        },
    );
    assert!(
        lines(&frames)
            .iter()
            .any(|l| l == "TEXT_MESSAGE_START evt-1 user")
    );
}

// ---- resume ------------------------------------------------------------------------------

fn fold(events: &[Event]) -> Projector {
    let mut projector = Projector::new(meta());
    for e in events {
        projector.apply(e, Audience::Viewer);
    }
    projector
}

#[test]
fn a_cursor_inside_a_run_gets_a_preamble_that_re_opens_the_run() {
    let events = asked();
    // After seq 2: working, the invocation open.
    let preamble = fold(&events[..2]).resume_preamble();
    assert_eq!(
        lines(&preamble),
        [
            "RUN_STARTED run-1",
            "SUBAGENT_STARTED sub-2 plain",
            "STATE_SNAPSHOT working"
        ]
    );
    assert!(preamble.iter().all(|f| f.resume_id.is_none()));
    // After seq 3: the invocation suspended already, the run not yet closed.
    assert_eq!(
        lines(&fold(&events[..3]).resume_preamble()),
        ["RUN_STARTED run-1", "STATE_SNAPSHOT working"]
    );
}

#[test]
fn a_cursor_between_runs_needs_no_preamble() {
    assert!(fold(&asked()).resume_preamble().is_empty());
    assert!(fold(&[]).resume_preamble().is_empty());
}

#[test]
fn the_stream_after_the_cursor_continues_the_run_the_preamble_re_opened() {
    let events = asked();
    let mut projector = fold(&events[..3]);
    let mut frames = projector.resume_preamble();
    frames.extend(projector.apply(&events[3], Audience::Viewer));
    verify::check(&frames).unwrap();
    assert_eq!(
        lines(&frames),
        [
            "RUN_STARTED run-1",
            "STATE_SNAPSHOT working",
            "STATE_SNAPSHOT blocked",
            "RUN_FINISHED run-1 interrupt[int-3:input_required @sub-2]  id:4",
        ]
    );
}

#[test]
fn a_cursor_in_the_middle_of_a_message_re_opens_it_with_what_was_said() {
    let events = vec![user(1, "go"), say(2, "m-1", "Plan", false)];
    let preamble = fold(&events).resume_preamble();
    let got = lines(&preamble);
    assert_eq!(got[3], "TEXT_MESSAGE_START m-1 assistant @sub-2");
    assert_eq!(got[4], "TEXT_MESSAGE_CONTENT m-1 \"Plan\"");
    // The stream stays well formed when the message then finishes.
    let mut projector = fold(&events);
    let mut frames = projector.resume_preamble();
    frames.extend(projector.apply(&say(3, "m-1", "Plan B", true), Audience::Viewer));
    verify::check(&frames).unwrap();
}

// ---- the view for translate --------------------------------------------------------------

#[test]
fn the_view_says_what_the_thread_holds() {
    use orch_agui_projection::ThreadView;
    let events = vec![
        user_ids(1, "go", "client-msg-1", "client-run-1"),
        status(2, AgentStatus::Working, None),
        say(3, "m-1", "hi", true),
        status(4, AgentStatus::InputRequired, Some("which?")),
        thread(5, ThreadState::Blocked),
    ];
    let ThreadView::Known(view) = fold(&events).view(&UserId::new("alice@example.com")) else {
        panic!("a known thread");
    };
    assert_eq!(view.state, ThreadState::Blocked);
    assert!(!view.run_open);
    assert_eq!(view.open_interrupts, ["int-4"]);
    assert_eq!(view.agent.as_str(), "plain");
    assert!(view.run_ids.contains("client-run-1"));
    for id in ["client-msg-1", "evt-2", "m-1", "evt-4"] {
        assert!(
            view.message_ids.contains(id),
            "{id}: {:?}",
            view.message_ids
        );
    }
}

#[test]
fn a_running_thread_has_a_run_open_and_nothing_to_answer() {
    use orch_agui_projection::ThreadView;
    let ThreadView::Known(view) = fold(&[user(1, "go")]).view(&UserId::new("a@b.c")) else {
        panic!("a known thread");
    };
    assert!(view.run_open);
    assert!(view.open_interrupts.is_empty());
}

// ---- a thread is a conversation (ADR 0020) ----------------------------------------------------

fn job_started(seq: i64, job: u32) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::JobStarted(orch_core::JobStartedData { job }),
    )
}

/// A job that ran to `done`, the log's first five events.
fn finished_job() -> Vec<Event> {
    vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::Completed, None),
        thread(4, ThreadState::Done),
    ]
}

#[test]
fn a_message_on_a_finished_thread_opens_the_next_jobs_run() {
    let mut events = finished_job();
    events.push(user(5, "and now this"));
    events.push(job_started(6, 2));
    events.push(status(7, AgentStatus::Working, None));
    events.push(status(8, AgentStatus::Completed, None));
    events.push(thread(9, ThreadState::Done));
    let got = all_lines(&events);
    let second: Vec<&String> = got
        .iter()
        .skip_while(|l| !l.starts_with("RUN_STARTED run-5"))
        .collect();
    assert_eq!(second[0], "RUN_STARTED run-5");
    // The run says which job it is from its first snapshot, and the boundary is an activity.
    let snapshot = project(&events)[4][1].clone();
    let orch_agui_proto::Event::StateSnapshot(s) = &snapshot.event else {
        panic!("{snapshot:?}");
    };
    assert_eq!(
        s.snapshot,
        serde_json::json!({"thread": {
            "jobNumber": 2, "state": "queued", "title": "a thread", "target": {"agentId": "plain"}
        }})
    );
    assert!(
        second
            .iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT job-2 vymalo.job {\"job\":2}")),
        "{second:?}"
    );
    assert_eq!(
        second.last().unwrap().as_str(),
        "RUN_FINISHED run-5 success  id:9"
    );
    // The first job's frames are what they were, with no job number.
    assert!(
        !got[..got.iter().position(|l| l == "RUN_STARTED run-5").unwrap()]
            .iter()
            .any(|l| l.contains("jobNumber"))
    );
}

#[test]
fn a_redelivered_message_opens_its_run_at_the_job_boundary() {
    // No `user_message`: it is in the log already, in the job before.
    let mut events = finished_job();
    events.push(job_started(5, 2));
    events.push(status(6, AgentStatus::Working, None));
    events.push(status(7, AgentStatus::Completed, None));
    events.push(thread(8, ThreadState::Done));
    let got = all_lines(&events);
    let second: Vec<&String> = got
        .iter()
        .skip_while(|l| !l.starts_with("RUN_STARTED run-5"))
        .collect();
    assert_eq!(second[0], "RUN_STARTED run-5");
    assert_eq!(second[1], "STATE_SNAPSHOT queued");
    assert!(second[2].starts_with("ACTIVITY_SNAPSHOT job-2 vymalo.job"));
}

#[test]
fn the_next_job_forgets_the_surfaces_the_attempt_and_the_commit_of_the_last() {
    use support::log::{gate, meta_under};
    let push = |seq: i64| {
        ev(
            seq,
            plain(),
            EventBody::Artifact(ArtifactData {
                name: "branch".to_owned(),
                mime_type: None,
                uri: None,
                text: Some(
                    serde_json::json!({
                        "repository": "https://github.com/o/r.git",
                        "branch": "agent/x",
                        "commit": "a".repeat(40)
                    })
                    .to_string(),
                ),
                file: None,
            }),
        )
    };
    let mut projector = Projector::new(meta_under(gate()));
    let mut feed = |e: Event| projector.apply(&e, Audience::Viewer);
    feed(user(1, "go"));
    feed(status(2, AgentStatus::Working, None));
    feed(push(3));
    feed(ev(
        4,
        Actor::system(),
        EventBody::Rework(orch_core::ReworkData {
            attempt: 2,
            max_attempts: 3,
            findings: vec![],
        }),
    ));
    feed(status(5, AgentStatus::Completed, None));
    let done = feed(ev(
        6,
        Actor::system(),
        EventBody::CheckResult(orch_core::CheckResult {
            source: orch_core::CheckSource::AgentChecks,
            name: None,
            attempt: 2,
            commit: Some("a".repeat(40)),
            status: orch_core::CheckStatus::Passed,
            summary: None,
            stale: false,
            findings: vec![],
        }),
    ));
    drop(done);
    feed(thread(7, ThreadState::Done));
    let opened = feed(user(8, "again"));
    let orch_agui_proto::Event::StateSnapshot(s) = &opened[1].event else {
        panic!("{opened:?}");
    };
    assert_eq!(
        s.snapshot["job"],
        serde_json::json!({"number": 2, "attempt": 1, "maxAttempts": 3, "gate": ["agent_checks"]})
    );
    assert_eq!(s.snapshot["thread"]["jobNumber"], 2);
    // An attempt 2 in job 2 has an id of its own (job 1's was `rework-2`).
    feed(job_started(9, 2));
    feed(status(10, AgentStatus::Working, None));
    let rework = feed(ev(
        11,
        Actor::system(),
        EventBody::Rework(orch_core::ReworkData {
            attempt: 2,
            max_attempts: 3,
            findings: vec![],
        }),
    ));
    assert!(
        lines(&rework)
            .iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT rework-j2-2 vymalo.rework")),
        "{:?}",
        lines(&rework)
    );
}

// ---- the agent's words, the time of an activity, typed artifacts --------------------------

fn named_artifact(seq: i64, name: &str, uri: Option<&str>, text: Option<&str>) -> Event {
    ev(
        seq,
        plain(),
        EventBody::Artifact(ArtifactData {
            name: name.to_owned(),
            mime_type: None,
            uri: uri.map(str::to_owned),
            text: text.map(str::to_owned),
            file: None,
        }),
    )
}

/// The content of the activity `id` in `frames`.
fn content_of(frames: &[Frame], id: &str) -> serde_json::Value {
    frames
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) if e.message_id.as_str() == id => {
                Some(serde_json::to_value(&e.content).unwrap())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no activity {id}"))
}

#[test]
fn the_words_of_a_completed_status_are_an_assistant_message_before_the_status() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, Some("Reading the repository")),
        status(
            3,
            AgentStatus::Completed,
            Some("Done: the login is **fixed**."),
        ),
        thread(4, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert_eq!(
        got[5..],
        [
            "SUBAGENT_STARTED sub-2 plain",
            // A working status keeps its detail: it is a step, not an answer.
            "ACTIVITY_SNAPSHOT evt-2 vymalo.status {\"detail\":\"Reading the repository\",\"status\":\"working\"} @sub-2",
            "STATE_SNAPSHOT working  id:2",
            "TEXT_MESSAGE_START st-3 assistant @sub-2",
            "TEXT_MESSAGE_CONTENT st-3 \"Done: the login is **fixed**.\"",
            "TEXT_MESSAGE_END st-3",
            "ACTIVITY_SNAPSHOT evt-3 vymalo.status {\"status\":\"completed\"} @sub-2",
            "SUBAGENT_FINISHED sub-2 success  id:3",
            "STATE_SNAPSHOT done",
            "RUN_FINISHED run-1 success  id:4",
        ]
    );
    // The message is the agent's, by name.
    let frames = support::flatten(&project(&events));
    let start = frames
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::TextMessageStart(e) if e.message_id.as_str() == "st-3" => {
                Some(e.clone())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(start.name.as_deref(), Some("plain"));
    assert_eq!(
        serde_json::to_value(start.base.metadata.unwrap()).unwrap()["vymalo.actor"]["name"],
        "plain"
    );
}

#[test]
fn a_status_that_repeats_the_last_final_message_says_nothing_more() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        say(3, "m1", "All done.", true),
        status(4, AgentStatus::Completed, Some("  All done.\n")),
        thread(5, ThreadState::Done),
    ];
    let got = all_lines(&events);
    assert!(!got.iter().any(|l| l.contains("st-4")), "{got:#?}");
    assert!(got.contains(
        &"ACTIVITY_SNAPSHOT evt-4 vymalo.status {\"status\":\"completed\"} @sub-2".to_owned()
    ));
    // Other words are said; so is a question after an answer in another invocation.
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        say(3, "m1", "Working on it.", true),
        status(4, AgentStatus::Completed, Some("All done.")),
        thread(5, ThreadState::Done),
    ];
    assert!(all_lines(&events).contains(&"TEXT_MESSAGE_CONTENT st-4 \"All done.\"".to_owned()));
    // A blank detail is no message, and a failure stays an error.
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Completed, Some("  ")),
        thread(3, ThreadState::Done),
    ];
    assert!(
        !all_lines(&events)
            .iter()
            .any(|l| l.starts_with("TEXT_MESSAGE_START st-"))
    );
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Failed, Some("boom")),
        thread(3, ThreadState::Failed),
    ];
    let got = all_lines(&events);
    assert!(!got.iter().any(|l| l.starts_with("TEXT_MESSAGE_START st-")));
    assert!(got.contains(&"ACTIVITY_SNAPSHOT evt-2 vymalo.status {\"detail\":\"boom\",\"status\":\"failed\"} @sub-2".to_owned()));
}

#[test]
fn a_status_closes_a_message_left_open_before_it_speaks() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        say(3, "m1", "Half a", false),
        status(4, AgentStatus::Completed, Some("The answer.")),
        thread(5, ThreadState::Done),
    ];
    let got = all_lines(&events);
    let end = got.iter().position(|l| l == "TEXT_MESSAGE_END m1").unwrap();
    let start = got
        .iter()
        .position(|l| l == "TEXT_MESSAGE_START st-4 assistant @sub-2")
        .unwrap();
    assert!(end < start, "{got:#?}");
}

#[test]
fn every_vymalo_activity_says_when_its_event_happened() {
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        artifact(3),
        error(4, "slow", true),
        thread(5, ThreadState::Blocked),
    ];
    let frames = support::flatten(&project(&events));
    let mut seen = 0;
    for frame in &frames {
        if let orch_agui_proto::Event::ActivitySnapshot(e) = &frame.event {
            let seq: i64 = e
                .message_id
                .as_str()
                .trim_start_matches("evt-")
                .parse()
                .unwrap();
            let want = Timestamp::from_second(1_800_000_000 + seq)
                .unwrap()
                .to_string();
            assert_eq!(
                e.content["at"],
                serde_json::Value::from(want),
                "{}",
                e.message_id
            );
            seen += 1;
        }
    }
    assert_eq!(seen, 3);
}

#[test]
fn an_artifact_says_its_kind_and_the_fields_a_card_needs() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let events = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        named_artifact(
            3,
            "branch",
            None,
            Some(&format!(
                r#"{{"repository":"https://github.com/Acme/Demo.git","branch":"agent/fix","commit":"{sha}"}}"#
            )),
        ),
        named_artifact(
            4,
            "checks",
            None,
            Some(&format!(
                r#"{{"passed":false,"commit":"{sha}","findings":["x"]}}"#
            )),
        ),
        named_artifact(
            5,
            "pull_request",
            None,
            Some(
                r#"{"url":"https://github.com/acme/demo/pull/12","number":"12","branch":"agent/fix"}"#,
            ),
        ),
        named_artifact(
            6,
            "Pull request",
            Some("https://github.com/acme/demo/pull/13"),
            None,
        ),
        named_artifact(7, "branch", None, Some("not json")),
        artifact(8),
    ];
    let frames = support::flatten(&project(&events));
    let strip = |mut v: serde_json::Value| {
        v.as_object_mut().unwrap().remove("at");
        v.as_object_mut().unwrap().remove("text");
        v
    };
    assert_eq!(
        strip(content_of(&frames, "evt-3")),
        serde_json::json!({"kind": "branch", "name": "branch", "repository": "github.com/acme/demo",
            "branch": "agent/fix", "sha": sha, "shortSha": "0123456"})
    );
    assert_eq!(
        strip(content_of(&frames, "evt-4")),
        serde_json::json!({"kind": "checks", "name": "checks", "passed": false, "sha": sha,
            "shortSha": "0123456"})
    );
    assert_eq!(
        strip(content_of(&frames, "evt-5")),
        serde_json::json!({"kind": "pull_request", "name": "pull_request",
            "url": "https://github.com/acme/demo/pull/12", "number": 12,
            "repository": "github.com/acme/demo", "branch": "agent/fix"})
    );
    assert_eq!(
        strip(content_of(&frames, "evt-6")),
        serde_json::json!({"kind": "pull_request", "name": "Pull request",
            "uri": "https://github.com/acme/demo/pull/13",
            "url": "https://github.com/acme/demo/pull/13", "number": 13,
            "repository": "github.com/acme/demo"})
    );
    // An artifact that cannot be used, and any other, is a file; its text is kept as sent.
    let broken = content_of(&frames, "evt-7");
    assert_eq!(
        (broken["kind"].as_str(), broken["text"].as_str()),
        (Some("file"), Some("not json"))
    );
    assert_eq!(content_of(&frames, "evt-8")["kind"], "file");
}

fn kept_file(seq: i64, name: &str, mime: &str, filename: Option<&str>, size: u64) -> Event {
    ev(
        seq,
        plain(),
        EventBody::Artifact(ArtifactData {
            name: name.to_owned(),
            mime_type: Some(mime.to_owned()),
            uri: None,
            text: None,
            file: Some(orch_core::FileRef {
                sha256: format!("{seq:064x}"),
                size,
                filename: filename.map(str::to_owned),
            }),
        }),
    )
}

/// ADR 0032: a file the artifact store keeps is a `file` with where to fetch it, how big it is and
/// whether a person can look at it without downloading it.
#[test]
fn a_kept_file_says_where_it_is_how_big_and_what_a_preview_of_it_is() {
    let events = vec![
        user(1, "make a chart"),
        status(2, AgentStatus::Working, None),
        kept_file(3, "chart", "image/png", Some("chart.png"), 1234),
        kept_file(4, "notes", "text/plain", Some("notes.txt"), 17),
        kept_file(5, "report", "application/pdf", None, 90_000),
        kept_file(6, "drawing", "image/svg+xml", Some("d.svg"), 300),
        kept_file(7, "data", "application/json", Some("d.json"), 2),
        // a file that was not kept is an entry without a file and says nothing of one
        named_artifact(8, "dump", None, None),
    ];
    let frames = support::flatten(&project(&events));
    let strip = |mut v: serde_json::Value| {
        v.as_object_mut().unwrap().remove("at");
        v
    };
    let href = |seq: i64| format!("/api/threads/{THREAD}/artifacts/{seq:064x}",);
    assert_eq!(
        strip(content_of(&frames, "evt-3")),
        serde_json::json!({"kind": "file", "name": "chart", "mimeType": "image/png",
            "href": href(3), "sha256": format!("{:064x}", 3), "size": 1234,
            "filename": "chart.png", "preview": "image"})
    );
    let notes = content_of(&frames, "evt-4");
    assert_eq!(
        (notes["preview"].as_str(), notes["size"].as_u64()),
        (Some("text"), Some(17))
    );
    let report = content_of(&frames, "evt-5");
    assert!(report["preview"].is_null(), "{report}");
    assert!(report.get("filename").is_none(), "no name was given");
    assert_eq!(report["href"], href(5));
    assert_eq!(content_of(&frames, "evt-6")["preview"], "image");
    assert_eq!(content_of(&frames, "evt-7")["preview"], "text");
    let refused = content_of(&frames, "evt-8");
    assert_eq!(refused["kind"], "file");
    assert!(
        refused.get("href").is_none() && refused.get("size").is_none(),
        "{refused}"
    );
}

// ---- what the words are for (ADR 0031) -----------------------------------------------------

fn say_as(
    seq: i64,
    id: &str,
    text: &str,
    purpose: Option<MessagePurpose>,
    via: Option<AnswerVia>,
) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentMessage(AgentMessageData {
            purpose,
            via,
            ..AgentMessageData::plain(id, text)
        }),
    )
}

/// The metadata of the `START` of the message `id`, in the frames of a projection.
fn start_metadata(frames: &[Vec<Frame>], id: &str) -> serde_json::Map<String, serde_json::Value> {
    support::flatten(frames)
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::TextMessageStart(s) if s.message_id.as_str() == id => {
                Some(s.base.metadata.clone().unwrap_or_default())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no TEXT_MESSAGE_START for {id}"))
        .into_iter()
        .collect()
}

#[test]
fn a_message_says_what_its_words_are_for_on_its_start() {
    let frames = project(&[
        user(1, "go"),
        say_as(2, "w", "Let me look.", Some(MessagePurpose::Working), None),
        say_as(3, "a", "Done.", Some(MessagePurpose::Answer), None),
        status(4, AgentStatus::Completed, Some("Done.")),
        thread(5, ThreadState::Done),
    ]);
    let working = start_metadata(&frames, "w");
    assert_eq!(working["vymalo.purpose"], "working");
    assert!(!working.contains_key("vymalo.via"));
    assert_eq!(working["vymalo.actor"]["name"], "plain");
    let answer = start_metadata(&frames, "a");
    assert_eq!(answer["vymalo.purpose"], "answer");
    assert!(!answer.contains_key("vymalo.via"));
}

#[test]
fn an_announced_answer_names_how() {
    let frames = project(&[
        user(1, "go"),
        say_as(
            2,
            "a",
            "Done.",
            Some(MessagePurpose::Answer),
            Some(AnswerVia::TurnOutput),
        ),
        status(3, AgentStatus::Completed, None),
        thread(4, ThreadState::Done),
    ]);
    let answer = start_metadata(&frames, "a");
    assert_eq!(answer["vymalo.purpose"], "answer");
    assert_eq!(answer["vymalo.via"], "turn_output");
}

#[test]
fn after_an_announced_answer_the_closing_words_are_working_text_said_once() {
    // what the core writes when an agent announces its answer and then ends the turn with a short
    // line: the announcement, the line as a working message ahead of the status that keeps it
    let frames = project(&[
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        say_as(
            3,
            "out-j-1",
            "The result.",
            Some(MessagePurpose::Answer),
            Some(AnswerVia::TurnOutput),
        ),
        say_as(
            4,
            "out-j-words-1",
            "Done; see above.",
            Some(MessagePurpose::Working),
            None,
        ),
        status(5, AgentStatus::Completed, Some("Done; see above.")),
        thread(6, ThreadState::Done),
    ]);
    let starts: Vec<(String, String)> = support::flatten(&frames)
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::TextMessageStart(s)
                if s.role == Some(orch_agui_proto::TextMessageRole::Assistant) =>
            {
                Some((
                    s.message_id.as_str().to_owned(),
                    s.base
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("vymalo.purpose"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                ))
            }
            _ => None,
        })
        .collect();
    // two messages, the answer and the working line; the status words are not said again as an
    // unmarked `st-5` that a reader would take for the answer
    assert_eq!(
        starts,
        [
            ("out-j-1".to_owned(), "answer".to_owned()),
            ("out-j-words-1".to_owned(), "working".to_owned()),
        ]
    );
    assert_eq!(
        start_metadata(&frames, "out-j-1")["vymalo.via"],
        "turn_output"
    );
}

#[test]
fn a_message_with_no_purpose_has_no_member_and_neither_have_the_status_words() {
    let frames = project(&[
        user(1, "go"),
        say(2, "m", "Plan.", true),
        status(3, AgentStatus::Completed, Some("All done.")),
        thread(4, ThreadState::Done),
    ]);
    for id in ["m", "st-3"] {
        let meta = start_metadata(&frames, id);
        assert!(!meta.contains_key("vymalo.purpose"), "{id}: {meta:?}");
        assert!(!meta.contains_key("vymalo.via"), "{id}: {meta:?}");
        assert_eq!(meta["vymalo.actor"]["name"], "plain");
    }
}

#[test]
fn a_message_that_is_open_when_a_connection_joins_is_opened_again_with_its_purpose() {
    // a partial (the legacy shape) is open across the cut; the preamble says it again
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        ev(
            3,
            plain(),
            EventBody::AgentMessage(AgentMessageData {
                is_final: false,
                purpose: Some(MessagePurpose::Working),
                ..AgentMessageData::plain("w", "Let me")
            }),
        ),
    ];
    let mut projector = Projector::new(meta());
    for e in &events {
        projector.apply(e, Audience::Viewer);
    }
    let preamble = projector.resume_preamble();
    let start = preamble
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::TextMessageStart(s) => Some(s.clone()),
            _ => None,
        })
        .expect("the open message is opened again");
    assert_eq!(start.base.metadata.unwrap()["vymalo.purpose"], "working");
}
