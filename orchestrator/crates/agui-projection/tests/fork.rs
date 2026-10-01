//! A fork (`thread_forked`, ADR 0029) in the projection: the copied events say what they said, and
//! the event that ends the copy opens a run of its own with the marker `vymalo.fork`, closes
//! whatever the copy left open, forgets the finished job and gives every later snapshot
//! `thread.forkedFrom`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector, ThreadView};
use orch_agui_proto::RunAgentInput;
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, AgentTarget, Event, EventBody, ForkKind,
    ForkSource, JobStartedData, ThreadForkedData, ThreadState, ThreadStateData, ThreadTitledData,
    Timestamp, TitledBy, UiSurfaceData, UserId, UserMessageData,
};
use serde_json::{Value, json};
use support::log::{catalog, meta, thread_id};
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

fn plain() -> Actor {
    Actor::agent(&AgentId::new("plain"), None)
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
        plain(),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: detail.map(str::to_owned),
        }),
    )
}

fn state(seq: i64, state: ThreadState) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData { state }),
    )
}

/// The fork of a parent (a thread with the same id here, which is all the projection needs),
/// cut at `cut`: the event at `cut + 1`.
fn forked(cut: i64, kind: ForkKind) -> Event {
    ev(
        cut + 1,
        Actor::user(&alice()),
        EventBody::ThreadForked(ThreadForkedData {
            from: ForkSource {
                thread_id: thread_id(),
                seq: cut,
            },
            kind,
            title: "Fix the redirect loop".to_owned(),
            target: AgentTarget {
                agent_id: AgentId::new("plain"),
                release: None,
            },
        }),
    )
}

/// A finished turn: a message, the agent's words, and the end.
fn finished_turn() -> Vec<Event> {
    vec![
        user(1, "fix the loop"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::Completed, Some("fixed")),
        state(4, ThreadState::Done),
    ]
}

/// Projects the whole log for a viewer, checking the stream as it goes; one entry per event.
fn project(events: &[Event]) -> (Projector, Vec<Vec<Frame>>) {
    let mut projector = Projector::new(meta());
    let mut checker = verify::Checker::new();
    let per_event: Vec<Vec<Frame>> = events
        .iter()
        .map(|e| {
            let frames = projector.apply(e, Audience::Viewer);
            checker
                .feed_frames(&frames)
                .unwrap_or_else(|why| panic!("seq {}: {why}", e.seq));
            frames
        })
        .collect();
    (projector, per_event)
}

fn snapshots(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::StateSnapshot(s) => Some(s.snapshot.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_fork_opens_a_run_of_its_own_with_the_marker_and_finishes_it() {
    let mut log = finished_turn();
    log.push(forked(4, ForkKind::Fork));
    let (_, per_event) = project(&log);
    let marker_line = format!(
        r#"ACTIVITY_SNAPSHOT fork-5 vymalo.fork {{"from":{{"seq":4,"threadId":"{}"}},"kind":"fork","target":{{"agentId":"plain"}},"title":"Fix the redirect loop"}}"#,
        thread_id()
    );
    assert_eq!(
        lines(&per_event[4]),
        [
            "RUN_STARTED run-5",
            marker_line.as_str(),
            "STATE_SNAPSHOT done",
            "RUN_FINISHED run-5 success  id:5",
        ]
    );
    // The activity says when, like every `vymalo.*` activity.
    let marker = per_event[4]
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(a) => Some(a.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(marker.content["at"], log[4].at.to_string());
    assert_eq!(
        marker.base.metadata.unwrap()["vymalo.actor"],
        json!({"type": "user", "name": "alice@example.com"})
    );
    // And the snapshot says where the thread came from, in the shape of `Thread.forkedFrom`, and
    // the title the fork has.
    let snapshot = &snapshots(&per_event[4])[0];
    assert_eq!(snapshot["thread"]["state"], "done");
    assert_eq!(snapshot["thread"]["title"], "Fix the redirect loop");
    assert_eq!(
        snapshot["thread"]["forkedFrom"],
        json!({"threadId": thread_id(), "seq": 4, "kind": "fork"})
    );
}

#[test]
fn the_snapshots_before_the_marker_are_the_parents_and_every_one_after_it_says_forked_from() {
    let mut log = finished_turn();
    log.push(forked(4, ForkKind::Edit));
    log.push(user(6, "go on"));
    log.push(status(7, AgentStatus::Working, None));
    log.push(ev(
        8,
        Actor::user(&alice()),
        EventBody::ThreadTitled(ThreadTitledData {
            title: "Mine".to_owned(),
            source: TitledBy::User,
        }),
    ));
    let (_, per_event) = project(&log);
    for (i, frames) in per_event.iter().enumerate() {
        for snapshot in snapshots(frames) {
            let has = snapshot["thread"].get("forkedFrom").is_some();
            assert_eq!(has, i >= 4, "event {}: {snapshot}", i + 1);
        }
    }
    // A rename after the fork keeps saying it.
    let renamed = snapshots(&per_event[7]);
    assert_eq!(renamed[0]["thread"]["title"], "Mine");
    assert_eq!(renamed[0]["thread"]["forkedFrom"]["kind"], "edit");
}

#[test]
fn the_next_message_of_a_fork_starts_the_next_job() {
    let mut log = finished_turn();
    log.push(forked(4, ForkKind::Fork));
    log.push(user(6, "go on"));
    log.push(ev(
        7,
        Actor::system(),
        EventBody::JobStarted(JobStartedData { job: 2 }),
    ));
    let (_, per_event) = project(&log);
    let message = lines(&per_event[5]);
    assert_eq!(message[0], "RUN_STARTED run-6");
    assert_eq!(message[1], "STATE_SNAPSHOT queued");
    let first = &snapshots(&per_event[5])[0];
    assert_eq!(first["thread"]["jobNumber"], 2);
    assert_eq!(first["thread"]["title"], "Fix the redirect loop");
    assert_eq!(first["thread"]["forkedFrom"]["seq"], 4);
    assert_eq!(
        lines(&per_event[6])[0],
        r#"ACTIVITY_SNAPSHOT job-2 vymalo.job {"job":2}  id:7"#
    );
}

#[test]
fn a_fork_after_job_two_goes_on_with_job_three() {
    let mut log = finished_turn();
    log.extend([
        user(5, "more"),
        ev(
            6,
            Actor::system(),
            EventBody::JobStarted(JobStartedData { job: 2 }),
        ),
        status(7, AgentStatus::Working, None),
        status(8, AgentStatus::Completed, None),
        state(9, ThreadState::Done),
        forked(9, ForkKind::Fork),
        user(11, "again"),
    ]);
    let (_, per_event) = project(&log);
    let snapshot = &snapshots(&per_event[9])[0];
    assert_eq!(
        snapshot["thread"]["jobNumber"], 2,
        "the fork's own: still job 2"
    );
    assert_eq!(
        snapshots(&per_event[10])[0]["thread"]["jobNumber"],
        3,
        "its next message starts job 3"
    );
}

#[test]
fn a_run_the_copy_left_open_is_closed_as_cancelled_before_the_fork_opens_its_own() {
    // The cut came before a message that was sent while the agent worked: the copy ends inside
    // its run.
    let log = vec![
        user(1, "fix the loop"),
        status(2, AgentStatus::Working, None),
        forked(2, ForkKind::Edit),
    ];
    let (projector, per_event) = project(&log);
    let got = lines(&per_event[2]);
    assert_eq!(
        &got[..3],
        [
            "SUBAGENT_FINISHED sub-2 success result={\"status\":\"canceled\"}",
            "STATE_SNAPSHOT done",
            "RUN_FINISHED run-1 cancelled",
        ]
    );
    assert_eq!(got[3], "RUN_STARTED run-3");
    assert!(got[4].starts_with("ACTIVITY_SNAPSHOT fork-3 vymalo.fork"));
    assert_eq!(
        &got[5..],
        ["STATE_SNAPSHOT done", "RUN_FINISHED run-3 success  id:3"]
    );
    assert!(!projector.run_open());
    assert_eq!(projector.thread_state(), ThreadState::Done);
}

#[test]
fn a_question_the_copy_still_holds_is_not_an_open_interrupt_of_the_fork() {
    let log = vec![
        user(1, "ask me"),
        status(2, AgentStatus::Working, None),
        status(3, AgentStatus::InputRequired, Some("Which branch?")),
        state(4, ThreadState::Blocked),
        forked(4, ForkKind::Fork),
    ];
    let (projector, per_event) = project(&log);
    // The parent's run ended in its interrupt; the fork's own ends in success.
    assert!(
        lines(&per_event[3])
            .last()
            .unwrap()
            .starts_with("RUN_FINISHED run-1 interrupt")
    );
    assert_eq!(
        lines(&per_event[4]).last().unwrap(),
        "RUN_FINISHED run-5 success  id:5"
    );
    let ThreadView::Known(view) = projector.view(&alice()) else {
        panic!("a known thread")
    };
    assert_eq!(view.state, ThreadState::Done);
    assert!(view.open_interrupts.is_empty());

    // A `resume` of the copied interrupt is ignored, and the message that comes with it starts
    // the next job.
    let input = RunAgentInput::from_value(json!({
        "threadId": support::log::THREAD,
        "runId": "run-x",
        "messages": [{"id": "m-new", "role": "user", "content": "main"}],
        "resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}}],
    }))
    .unwrap()
    .input;
    let got =
        orch_agui_projection::translate_with_warnings(&input, &ThreadView::Known(view)).unwrap();
    assert_eq!(got.inputs.len(), 1);
    assert!(matches!(
        got.warnings[..],
        [orch_agui_projection::Warning::ResumeIgnored { .. }]
    ));
}

#[test]
fn a_card_of_the_copy_cannot_be_acted_on() {
    let mut operations = vec![json!({
        "version": "v0.9.1",
        "createSurface": {"surfaceId": "s0", "catalogId": "c"},
    })];
    operations.push(json!({
        "version": "v0.9.1",
        "updateComponents": {"surfaceId": "s0", "components": [
            {"id": "root", "component": "Text", "text": "hi"}]},
    }));
    let log = vec![
        user(1, "ui"),
        status(2, AgentStatus::Working, None),
        ev(
            3,
            plain(),
            EventBody::UiSurface(UiSurfaceData { operations }),
        ),
        status(4, AgentStatus::Completed, None),
        state(5, ThreadState::Done),
    ];
    let (parent, _) = project(&log);
    // The parent has the card; the fork has none (an action names a surface the thread has, and
    // one it has not is a 422).
    let ThreadView::Known(before) = parent.view(&alice()) else {
        panic!()
    };
    assert!(before.surfaces.contains_key("s0"));
    let mut forked_log = log.clone();
    forked_log.push(forked(5, ForkKind::Fork));
    let (fork, _) = project(&forked_log);
    let ThreadView::Known(after) = fork.view(&alice()) else {
        panic!()
    };
    assert!(after.surfaces.is_empty());
}

#[test]
fn what_the_copy_held_in_ids_is_still_known_so_the_history_a_screen_sends_is_accepted() {
    let mut log = finished_turn();
    log.push(forked(4, ForkKind::Fork));
    let (projector, _) = project(&log);
    let ThreadView::Known(view) = projector.view(&alice()) else {
        panic!()
    };
    // The user message, the agent's words (`st-3`), the status activity and the marker.
    for id in ["evt-1", "st-3", "evt-2", "evt-3", "fork-5"] {
        assert!(
            view.message_ids.contains(id),
            "{id}: {:?}",
            view.message_ids
        );
    }
    assert!(view.run_ids.contains("run-1") && view.run_ids.contains("run-5"));
}

#[test]
fn a_fork_forgets_the_parents_ui_catalog_which_its_agent_was_never_sent() {
    let log = vec![
        ev(1, Actor::user(&alice()), EventBody::UiCatalog(catalog(0))),
        user(2, "hi"),
        status(3, AgentStatus::Completed, None),
        state(4, ThreadState::Done),
        forked(4, ForkKind::Fork),
    ];
    let (_, per_event) = project(&log);
    assert!(
        snapshots(&per_event[1])[0]["thread"]
            .get("uiCatalog")
            .is_some()
    );
    assert!(
        snapshots(&per_event[4])[0]["thread"]
            .get("uiCatalog")
            .is_none(),
        "the fork has none: the next message that carries one sends it in full"
    );
}
