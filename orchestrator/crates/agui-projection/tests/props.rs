//! Properties over random legal logs (logs the core's `transition` can produce), with and
//! without a verification gate: the projected stream is well formed at every prefix, and a client
//! that reconnects from any resume point gets exactly the rest.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_core::{Event, EventBody, ThreadState};
use proptest::prelude::*;
use support::log::{arb_actions, world};
use support::{flatten, lines, verify};

fn is_active(state: ThreadState) -> bool {
    matches!(
        state,
        ThreadState::Queued | ThreadState::Working | ThreadState::Verifying
    )
}

proptest! {
    /// The stream is well formed at every prefix of the log (each prefix is a prefix of the
    /// stream), and between the core's transactions a run is open exactly when the thread is
    /// queued, working or verifying (under a gate). (Inside one, between an event and the `thread_state` it announces,
    /// the run is still open: the `thread_state` is what closes it.)
    #[test]
    fn the_stream_is_well_formed_at_every_prefix(actions in arb_actions(), gated in any::<bool>()) {
        let (events, meta) = world(gated, &actions);
        let mut projector = Projector::new(meta.clone());
        let mut checker = verify::Checker::new();
        for (index, event) in events.iter().enumerate() {
            let announces_state = matches!(
                events.get(index + 1).map(|e| &e.body),
                Some(EventBody::ThreadState(_))
            );
            let frames = projector.apply(event, Audience::Viewer);
            if let Err(e) = checker.feed_frames(&frames) {
                return Err(TestCaseError::fail(format!(
                    "after seq {}: {e}\nframes so far are fine up to here; this event gave {:?}",
                    event.seq,
                    lines(&frames)
                )));
            }
            if !announces_state {
                prop_assert_eq!(
                    projector.run_open(),
                    is_active(projector.thread_state()),
                    "run open must mean queued, working or verifying (seq {}, state {:?})",
                    event.seq,
                    projector.thread_state()
                );
            }
            prop_assert_eq!(checker.run_open(), projector.run_open());
        }
    }

    /// Nothing is left open when a run ends, and a finished log ends outside any run.
    #[test]
    fn nothing_is_open_at_a_terminal_event(actions in arb_actions(), gated in any::<bool>()) {
        let (events, meta) = world(gated, &actions);
        let frames = flatten(&support::project_each_with(&events, meta.clone()));
        let mut checker = verify::Checker::new();
        for frame in &frames {
            checker.feed(&frame.event).map_err(TestCaseError::fail)?;
            if matches!(
                frame.event,
                orch_agui_proto::Event::RunFinished(_) | orch_agui_proto::Event::RunError(_)
            ) {
                prop_assert!(!checker.run_open() && !checker.text_open());
            }
        }
        if let Some(last) = events.last() {
            let mut projector = Projector::new(meta.clone());
            for e in &events {
                projector.apply(e, Audience::Viewer);
            }
            if !is_active(projector.thread_state()) {
                prop_assert!(!projector.run_open(), "ended by {:?}", last.body);
            }
        }
    }

    /// Resuming from any resume point gives exactly the frames that follow it: rebuilding the
    /// projector from the events up to the cursor and continuing produces the same suffix, and
    /// the preamble plus that suffix is a well-formed stream that starts with `RUN_STARTED`.
    #[test]
    fn resuming_from_any_resume_point_gives_exactly_the_suffix(actions in arb_actions(), gated in any::<bool>()) {
        let (events, meta) = world(gated, &actions);
        let per_event = support::project_each_with(&events, meta.clone());
        let full = flatten(&per_event);
        for (index, frames) in per_event.iter().enumerate() {
            let Some(cursor) = frames.last().and_then(|f| f.resume_id) else { continue };
            prop_assert_eq!(cursor, events[index].seq);

            // What the client already has: everything up to and including the cursor frame.
            let consumed: usize = per_event[..=index].iter().map(Vec::len).sum();
            let want: &[Frame] = &full[consumed..];

            let mut rebuilt = Projector::new(meta.clone());
            for e in &events[..=index] {
                rebuilt.apply(e, Audience::Viewer);
            }
            let preamble = rebuilt.resume_preamble();
            let mut got = Vec::new();
            for e in &events[index + 1..] {
                got.extend(rebuilt.apply(e, Audience::Viewer));
            }
            prop_assert_eq!(&got[..], want, "suffix after cursor {}", cursor);

            // What the client sees on the connection: preamble, then the suffix.
            let mut stream = preamble.clone();
            stream.extend(got);
            if let Err(e) = verify::check(&stream) {
                return Err(TestCaseError::fail(format!(
                    "reconnect at {cursor}: {e}\n{:#?}",
                    lines(&stream)
                )));
            }
            // A preamble exists exactly when a run is open at the cursor, and it re-opens that run.
            let open_at_cursor = full[..consumed].iter().rev().find_map(|f| match &f.event {
                orch_agui_proto::Event::RunStarted(e) => Some(Some(e.run_id.clone())),
                orch_agui_proto::Event::RunFinished(_) | orch_agui_proto::Event::RunError(_) => Some(None),
                _ => None,
            }).flatten();
            match (open_at_cursor, preamble.first().map(|f| &f.event)) {
                (Some(run), Some(orch_agui_proto::Event::RunStarted(e))) => prop_assert_eq!(run, e.run_id.clone()),
                (None, None) => {}
                (open, first) => prop_assert!(false, "run open at cursor {:?} but preamble starts {:?}", open, first),
            }
        }
    }

    /// Every resume point is a whole log event: ids strictly increase and each names the event
    /// that produced the frame.
    #[test]
    fn resume_ids_are_log_sequence_numbers_in_order(actions in arb_actions(), gated in any::<bool>()) {
        let (events, meta) = world(gated, &actions);
        let per_event = support::project_each_with(&events, meta.clone());
        let mut last = 0;
        for (event, frames) in events.iter().zip(&per_event) {
            for (i, frame) in frames.iter().enumerate() {
                if let Some(id) = frame.resume_id {
                    prop_assert_eq!(id, event.seq);
                    prop_assert_eq!(i, frames.len() - 1, "only the last frame of an event");
                    prop_assert!(id > last);
                    last = id;
                }
            }
        }
    }

    /// The requester's stream is the viewer's without the user messages it holds; nothing else
    /// differs (the state of the projection does not depend on the audience).
    #[test]
    fn the_requester_differs_from_the_viewer_only_in_the_messages_it_holds(actions in arb_actions(), gated in any::<bool>()) {
        let (events, meta) = world(gated, &actions);
        let held: BTreeSet<String> = events
            .iter()
            .filter_map(|e| match &e.body {
                EventBody::UserMessage(m) => m.message_id.clone(),
                _ => None,
            })
            .collect();
        let mut viewer = Projector::new(meta.clone());
        let mut requester = Projector::new(meta.clone());
        for event in &events {
            let v = viewer.apply(event, Audience::Viewer);
            let r = requester.apply(event, Audience::Requester { held_message_ids: &held });
            let held_here = matches!(&event.body, EventBody::UserMessage(m)
                if m.message_id.as_ref().is_some_and(|id| held.contains(id)));
            let kinds = |frames: &[Frame], skip_triad: bool| -> Vec<String> {
                frames
                    .iter()
                    .filter(|f| {
                        !(skip_triad
                            && matches!(
                                f.event,
                                orch_agui_proto::Event::TextMessageStart(_)
                                    | orch_agui_proto::Event::TextMessageContent(_)
                                    | orch_agui_proto::Event::TextMessageEnd(_)
                            ))
                    })
                    .map(|f| {
                        let mut f = f.clone();
                        f.resume_id = None;
                        serde_json::to_string(&f.event).unwrap()
                    })
                    .collect()
            };
            prop_assert_eq!(kinds(&r, false), kinds(&v, held_here));
        }
    }

    /// The projection is a function of the log: the same events give the same frames.
    #[test]
    fn the_projection_is_a_function_of_the_log(actions in arb_actions(), gated in any::<bool>()) {
        let (events, meta) = world(gated, &actions);
        prop_assert_eq!(support::project_each_with(&events, meta.clone()), support::project_each_with(&events, meta.clone()));
    }

    /// Every frame passes the conformance testkit (`assert_conforms`: valid against the vendored
    /// schema). The stream checker applies the same oracle; stated alone so a failure names it.
    #[test]
    fn every_frame_conforms_to_the_schema(actions in arb_actions(), gated in any::<bool>()) {
        let (events, meta) = world(gated, &actions);
        let events: Vec<Event> = events;
        for frame in flatten(&support::project_each_with(&events, meta.clone())) {
            orch_agui_proto::testkit::assert_conforms(&frame.event);
        }
    }
}
