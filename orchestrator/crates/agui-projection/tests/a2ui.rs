//! A2UI in the projection (ADR 0013): `ui_surface` becomes a whole-surface `a2ui-surface`
//! snapshot with `replace: true`, `ui_action` becomes a `vymalo.action` activity, and the
//! projection knows which surfaces the thread has.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{
    A2UI_OPERATIONS_KEY, ACTIVITY_A2UI_SURFACE, ACTIVITY_ACTION, Audience, Frame, Projector,
    ThreadView,
};
use orch_agui_proto::Event as Agui;
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, Event, EventBody, MAX_SURFACE_BYTES, ThreadState,
    ThreadStateData, Timestamp, UiActionData, UiSurfaceData, UiVersion, UserId, UserMessageData,
};
use serde_json::{Value, json};
use support::log::{THREAD, meta, surface_op, thread_id};
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

fn user(seq: i64) -> Event {
    ev(
        seq,
        alice(),
        EventBody::UserMessage(UserMessageData::new("show me")),
    )
}

fn working(seq: i64) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::Working,
            detail: None,
        }),
    )
}

fn asks(seq: i64) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::InputRequired,
            detail: Some("Pick one".to_owned()),
        }),
    )
}

fn blocked(seq: i64) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData {
            state: ThreadState::Blocked,
        }),
    )
}

fn surface(seq: i64, operations: Vec<Value>) -> Event {
    ev(
        seq,
        plain(),
        EventBody::UiSurface(UiSurfaceData { operations }),
    )
}

fn action(seq: i64, run_id: Option<&str>) -> Event {
    let mut context = serde_json::Map::new();
    context.insert("choice".into(), json!("a"));
    ev(
        seq,
        alice(),
        EventBody::UiAction(UiActionData {
            surface_id: "s0".into(),
            name: "go".into(),
            source_component_id: "btn".into(),
            context,
            version: UiVersion::V0_9_1,
            run_id: run_id.map(str::to_owned),
        }),
    )
}

fn project(events: &[Event]) -> (Projector, Vec<Frame>) {
    let mut projector = Projector::new(meta());
    let frames = events
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    (projector, frames)
}

/// The `a2ui-surface` snapshots of a stream, as `(messageId, replace, operations)`.
fn snapshots(frames: &[Frame]) -> Vec<(String, Option<bool>, Vec<Value>)> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            Agui::ActivitySnapshot(a) if a.activity_type == ACTIVITY_A2UI_SURFACE => Some((
                a.message_id.to_string(),
                a.replace,
                a.content[A2UI_OPERATIONS_KEY].as_array().unwrap().clone(),
            )),
            _ => None,
        })
        .collect()
}

fn known(projector: &Projector) -> orch_agui_projection::KnownThread {
    let ThreadView::Known(k) = projector.view(&UserId::new("alice@example.com")) else {
        panic!("a known thread");
    };
    k
}

#[test]
fn every_update_sends_the_whole_surface_again_under_one_message_id() {
    let create = surface_op(0, 0);
    let components = surface_op(0, 1);
    let data = surface_op(0, 2);
    let events = [
        user(1),
        working(2),
        surface(3, vec![create.clone(), components.clone()]),
        surface(4, vec![data.clone()]),
        asks(5),
        blocked(6),
    ];
    let (_, frames) = project(&events);
    let got = snapshots(&frames);
    assert_eq!(
        got,
        [
            (
                "a2ui-3".to_owned(),
                Some(true),
                vec![create.clone(), components.clone()]
            ),
            (
                "a2ui-3".to_owned(),
                Some(true),
                vec![create, components, data]
            ),
        ],
        "the message id is that of the event that created the surface; each snapshot is whole"
    );
    verify::check(&frames).unwrap();
}

#[test]
fn a_snapshot_is_attributed_to_the_agent_and_its_invocation() {
    let (_, frames) = project(&[user(1), working(2), surface(3, vec![surface_op(0, 0)])]);
    let Some(Agui::ActivitySnapshot(a)) = frames.iter().find_map(|f| match &f.event {
        e @ Agui::ActivitySnapshot(a) if a.activity_type == ACTIVITY_A2UI_SURFACE => {
            Some(e.clone())
        }
        _ => None,
    }) else {
        panic!("no surface snapshot");
    };
    assert_eq!(
        a.subagent_run_id.as_ref().map(ToString::to_string),
        Some("sub-2".into())
    );
    let actor = &a.base.metadata.as_ref().unwrap()["vymalo.actor"];
    assert_eq!(actor["type"], "agent");
    assert_eq!(actor["name"], "plain");
}

#[test]
fn a_payload_for_two_surfaces_is_two_snapshots_in_order_of_appearance() {
    let (_, frames) = project(&[
        user(1),
        working(2),
        surface(
            3,
            vec![
                surface_op(1, 0),
                surface_op(0, 0),
                surface_op(1, 1),
                surface_op(0, 1),
            ],
        ),
    ]);
    let got = snapshots(&frames);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, "a2ui-3");
    assert_eq!(got[0].2, [surface_op(1, 0), surface_op(1, 1)]);
    assert_eq!(got[1].2, [surface_op(0, 0), surface_op(0, 1)]);
    assert_ne!(got[0].2, got[1].2);
    verify::check(&frames).unwrap();
}

#[test]
fn two_surfaces_of_one_thread_have_their_own_message_ids() {
    let (_, frames) = project(&[
        user(1),
        working(2),
        surface(3, vec![surface_op(0, 0)]),
        surface(4, vec![surface_op(1, 0)]),
        surface(5, vec![surface_op(0, 1)]),
    ]);
    let ids: Vec<String> = snapshots(&frames).into_iter().map(|s| s.0).collect();
    assert_eq!(ids, ["a2ui-3", "a2ui-4", "a2ui-3"]);
}

#[test]
fn a_deleted_surface_is_sent_with_its_delete_and_a_new_one_starts_afresh() {
    let (projector, frames) = project(&[
        user(1),
        working(2),
        surface(3, vec![surface_op(0, 0), surface_op(0, 1)]),
        surface(4, vec![surface_op(0, 3)]),
        surface(5, vec![surface_op(0, 0)]),
    ]);
    let got = snapshots(&frames);
    assert_eq!(got.len(), 3);
    assert_eq!(
        got[1].2,
        [surface_op(0, 0), surface_op(0, 1), surface_op(0, 3)],
        "the old surface's last snapshot ends in the delete"
    );
    assert_eq!(got[1].0, "a2ui-3");
    assert_eq!(got[2].0, "a2ui-5", "the recreated surface is a new message");
    assert_eq!(got[2].2, [surface_op(0, 0)], "and starts from nothing");
    assert!(known(&projector).surfaces.contains_key("s0"));
}

#[test]
fn a_delete_and_a_create_in_one_payload_are_two_snapshots_of_two_surfaces() {
    let (_, frames) = project(&[
        user(1),
        working(2),
        surface(3, vec![surface_op(0, 0)]),
        surface(
            4,
            vec![surface_op(0, 3), surface_op(0, 0), surface_op(0, 1)],
        ),
    ]);
    let got = snapshots(&frames);
    assert_eq!(got.len(), 3);
    assert_eq!(got[1].0, "a2ui-3");
    assert_eq!(got[1].2.last().unwrap(), &surface_op(0, 3));
    assert_eq!(got[2].0, "a2ui-4");
    assert_eq!(got[2].2, [surface_op(0, 0), surface_op(0, 1)]);
}

#[test]
fn the_projection_knows_the_surfaces_the_thread_has_and_their_version() {
    let v10 = json!({"version": "v1.0", "createSurface": {"surfaceId": "late"}});
    let (projector, _) = project(&[
        user(1),
        working(2),
        surface(3, vec![surface_op(0, 0), v10]),
        asks(4),
        blocked(5),
    ]);
    let k = known(&projector);
    assert_eq!(k.surfaces.len(), 2);
    assert_eq!(k.surfaces["s0"], UiVersion::V0_9_1);
    assert_eq!(k.surfaces["late"], UiVersion::V1_0);
    // Activity ids join the ids the thread holds, so a client that echoes them back is not
    // sending a new message.
    assert!(k.message_ids.contains("a2ui-3"));

    let (projector, _) = project(&[
        user(1),
        working(2),
        surface(3, vec![surface_op(0, 0), surface_op(0, 3)]),
    ]);
    assert!(
        known(&projector).surfaces.is_empty(),
        "a deleted surface is gone"
    );
}

#[test]
fn an_operation_that_fails_the_envelope_is_never_relayed() {
    // The log holds only checked payloads; a foreign event with junk in it is skipped.
    let junk = vec![
        json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s0"}}),
        json!({"nonsense": true}),
        json!("a string"),
    ];
    let (_, frames) = project(&[user(1), working(2), surface(3, junk)]);
    let got = snapshots(&frames);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].2.len(), 1, "{:?}", got[0].2);
    let (_, frames) = project(&[user(1), working(2), surface(3, vec![json!(1)])]);
    assert!(snapshots(&frames).is_empty());
}

#[test]
fn a_surface_that_outgrows_the_replay_cap_stops_and_the_viewer_is_told_once() {
    let filler = |n: usize| {
        json!({"version": "v0.9.1", "updateDataModel": {
            "surfaceId": "s0", "value": "x".repeat(MAX_SURFACE_BYTES / 2 - 200 + n)}})
    };
    let (_, frames) = project(&[
        user(1),
        working(2),
        surface(3, vec![surface_op(0, 0), filler(0)]),
        surface(4, vec![filler(1)]),
        surface(5, vec![filler(2)]),
        surface(6, vec![filler(3)]),
    ]);
    let got = snapshots(&frames);
    assert_eq!(
        got.len(),
        2,
        "the snapshots stop when the surface stops fitting"
    );
    let errors: Vec<&Frame> = frames
        .iter()
        .filter(
            |f| matches!(&f.event, Agui::ActivitySnapshot(a) if a.activity_type == "vymalo.error"),
        )
        .collect();
    assert_eq!(errors.len(), 1, "{}", lines(&frames).join("\n"));
    verify::check(&frames).unwrap();
}

#[test]
fn a_surface_that_arrives_after_the_thread_blocked_is_a_run_of_its_own() {
    let (_, frames) = project(&[
        user(1),
        working(2),
        asks(3),
        blocked(4),
        surface(5, vec![surface_op(0, 0)]),
    ]);
    verify::check(&frames).unwrap();
    let run = frames.iter().rev().find_map(|f| match &f.event {
        Agui::RunStarted(r) => Some(r.run_id.to_string()),
        _ => None,
    });
    assert_eq!(run.as_deref(), Some("run-5"), "a producer-initiated run");
    let Agui::RunFinished(done) = &frames.last().unwrap().event else {
        panic!("the run is not closed");
    };
    assert!(
        matches!(
            done.outcome,
            Some(orch_agui_proto::RunFinishedOutcome::Interrupt { .. })
        ),
        "the thread still waits: the interrupt is raised again"
    );
}

#[test]
fn an_action_opens_a_run_under_its_run_id_and_says_what_was_done() {
    let (projector, frames) = project(&[
        user(1),
        working(2),
        surface(3, vec![surface_op(0, 0)]),
        asks(4),
        blocked(5),
        action(6, Some("run-act")),
    ]);
    let tail: Vec<Value> = frames
        .iter()
        .skip_while(|f| f.resume_id != Some(5))
        .skip(1)
        .map(|f| serde_json::to_value(&f.event).unwrap())
        .collect();
    assert_eq!(tail[0]["type"], "RUN_STARTED");
    assert_eq!(tail[0]["runId"], "run-act");
    assert_eq!(tail[0]["threadId"], THREAD);
    assert_eq!(tail[1]["type"], "STATE_SNAPSHOT");
    assert_eq!(tail[1]["snapshot"]["thread"]["state"], "queued");
    assert_eq!(tail[2]["type"], "ACTIVITY_SNAPSHOT");
    assert_eq!(tail[2]["activityType"], ACTIVITY_ACTION);
    assert_eq!(tail[2]["messageId"], "evt-6");
    assert_eq!(
        tail[2]["content"],
        json!({"surfaceId": "s0", "name": "go", "sourceComponentId": "btn",
               "context": {"choice": "a"}})
    );
    assert_eq!(tail[2]["metadata"]["vymalo.actor"]["type"], "user");
    assert_eq!(tail.len(), 3, "an action says nothing in the transcript");
    assert_eq!(frames.last().unwrap().resume_id, Some(6));
    assert!(projector.run_open());
    assert_eq!(projector.thread_state(), ThreadState::Queued);
    assert!(known(&projector).run_ids.contains("run-act"));
    assert!(
        known(&projector).open_interrupts.is_empty(),
        "the action answered the wait"
    );
    verify::check(&frames).unwrap();
}

#[test]
fn an_action_without_a_run_id_gets_the_run_of_its_event() {
    let (_, frames) = project(&[user(1), working(2), asks(3), blocked(4), action(5, None)]);
    let run = frames
        .iter()
        .rev()
        .find_map(|f| match &f.event {
            Agui::RunStarted(r) => Some(r.run_id.to_string()),
            _ => None,
        })
        .unwrap();
    assert_eq!(run, "run-5");
}

#[test]
fn an_action_inside_an_open_run_joins_it() {
    let (_, frames) = project(&[user(1), working(2), action(3, Some("ignored"))]);
    let starts = frames
        .iter()
        .filter(|f| matches!(f.event, Agui::RunStarted(_)))
        .count();
    assert_eq!(starts, 1);
    verify::check(&frames).unwrap();
}

#[test]
fn resuming_with_a_cursor_yields_exactly_the_rest() {
    // The whole surface is in every snapshot, so a client that holds only some of the stream
    // still ends with the complete surface.
    let events = [
        user(1),
        working(2),
        surface(3, vec![surface_op(0, 0)]),
        surface(4, vec![surface_op(0, 1)]),
        surface(5, vec![surface_op(0, 2)]),
        asks(6),
        blocked(7),
    ];
    let (_, all) = project(&events);
    let (_, upto) = project(&events[..4]);
    let mut projector = Projector::new(meta());
    for e in &events[..4] {
        let _ = projector.apply(e, Audience::Viewer);
    }
    let mut resumed: Vec<Frame> = projector.resume_preamble();
    for e in &events[4..] {
        resumed.extend(projector.apply(e, Audience::Viewer));
    }
    let last_of = |frames: &[Frame]| snapshots(frames).last().cloned();
    assert_eq!(last_of(&resumed), last_of(&all), "the same last snapshot");
    assert_eq!(last_of(&resumed).unwrap().2.len(), 3);
    assert!(upto.len() < all.len());
}
