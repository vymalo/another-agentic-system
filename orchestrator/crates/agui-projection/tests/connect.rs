//! The connect fold: what a client that attaches with a cursor is sent, and when a `?mode=run`
//! stream ends. Properties over random legal logs, and the edges by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Connect, Follow, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, Event, EventBody, ThreadState, ThreadStateData,
    Timestamp, UserId, UserMessageData,
};
use proptest::prelude::*;
use support::log::{arb_actions, build, meta, thread_id};
use support::{flatten, lines, project_each, verify};

fn run_to_end(mut connect: Connect, events: &[Event]) -> Vec<Frame> {
    let mut out = Vec::new();
    for event in events {
        out.extend(connect.feed(event));
    }
    out
}

fn ev(seq: i64, actor: Actor, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    }
}

fn user(seq: i64, text: &str) -> Event {
    ev(
        seq,
        Actor::user(&UserId::new("alice@example.com")),
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

fn state(seq: i64, state: ThreadState) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData { state }),
    )
}

proptest! {
    /// From any cursor, the stream is the preamble of the state at the cursor followed by the
    /// frames of every later event, exactly the frames an uninterrupted stream wrote after its
    /// frame with that cursor as `id:`; and preamble plus suffix is a well-formed stream.
    #[test]
    fn a_connect_from_any_cursor_is_the_preamble_and_the_suffix(actions in arb_actions()) {
        let events = build(&actions);
        let len = i64::try_from(events.len()).unwrap();
        let per_event = project_each(&events);
        let full = flatten(&per_event);
        for cursor in 0..=len {
            let at = usize::try_from(cursor).unwrap();
            let got = run_to_end(Connect::new(meta(), cursor, len, Follow::Forever), &events);

            let mut rebuilt = Projector::new(meta());
            for e in &events[..at] {
                rebuilt.apply(e, Audience::Viewer);
            }
            let preamble = rebuilt.resume_preamble();
            let consumed: usize = per_event[..at].iter().map(Vec::len).sum();
            let mut want = preamble.clone();
            want.extend_from_slice(&full[consumed..]);
            prop_assert_eq!(&got, &want, "cursor {}", cursor);

            if let Err(e) = verify::check(&got) {
                return Err(TestCaseError::fail(format!(
                    "cursor {cursor}: {e}\n{:#?}",
                    lines(&got)
                )));
            }
            // Only the preamble is not a resume point: every other id is a whole event.
            prop_assert!(got[..preamble.len()].iter().all(|f| f.resume_id.is_none()));
            for frame in &got[preamble.len()..] {
                if let Some(id) = frame.resume_id {
                    prop_assert!(id > cursor, "resume id {} after cursor {}", id, cursor);
                }
            }
        }
    }

    /// Reconnecting at the last resume point a client saw, and again at the next one, never
    /// repeats a frame and never skips one: the frames a client holds plus the connect from its
    /// cursor are the whole stream, once.
    #[test]
    fn what_was_held_plus_the_reconnect_is_the_stream_once(actions in arb_actions()) {
        let events = build(&actions);
        let len = i64::try_from(events.len()).unwrap();
        let per_event = project_each(&events);
        let full = flatten(&per_event);
        for (index, frames) in per_event.iter().enumerate() {
            let Some(cursor) = frames.last().and_then(|f| f.resume_id) else { continue };
            let held: usize = per_event[..=index].iter().map(Vec::len).sum();
            let got = run_to_end(Connect::new(meta(), cursor, len, Follow::Forever), &events);
            let mut rebuilt = Projector::new(meta());
            for e in &events[..=index] {
                rebuilt.apply(e, Audience::Viewer);
            }
            let preamble_len = rebuilt.resume_preamble().len();
            let mut joined = full[..held].to_vec();
            joined.extend_from_slice(&got[preamble_len..]);
            prop_assert_eq!(joined, full.clone(), "cursor {}", cursor);
        }
    }

    /// `?mode=run` writes what the default writes, and ends at the first point at or after the
    /// end of the log as it stood at connect time where no run is open: right after the replay
    /// when the thread is idle, and when the open run closes otherwise.
    #[test]
    fn mode_run_ends_where_the_open_run_closes(actions in arb_actions()) {
        let events = build(&actions);
        let len = events.len();
        // Whether a run is open after the first k events.
        let mut open_after = vec![false];
        let mut probe = Projector::new(meta());
        for e in &events {
            probe.apply(e, Audience::Viewer);
            open_after.push(probe.run_open());
        }
        for head in 0..=len {
            for cursor in 0..=head {
                let mut through = Connect::new(
                    meta(),
                    i64::try_from(cursor).unwrap(),
                    i64::try_from(head).unwrap(),
                    Follow::ThroughRun,
                );
                let mut forever = Connect::new(
                    meta(),
                    i64::try_from(cursor).unwrap(),
                    i64::try_from(head).unwrap(),
                    Follow::Forever,
                );
                prop_assert!(!forever.finished());
                let mut got = Vec::new();
                let mut want = Vec::new();
                let mut ended = through.finished().then_some(0);
                for (i, e) in events.iter().enumerate() {
                    if ended.is_some() {
                        break;
                    }
                    got.extend(through.feed(e));
                    want.extend(forever.feed(e));
                    prop_assert!(!forever.finished());
                    if through.finished() {
                        ended = Some(i + 1);
                    }
                }
                let expected_end = (head..=len).find(|k| !open_after[*k]);
                prop_assert_eq!(ended, expected_end, "head {} cursor {}", head, cursor);
                prop_assert_eq!(got, want, "head {} cursor {}", head, cursor);
            }
        }
    }
}

#[test]
fn a_cursor_beyond_the_log_waits_for_what_comes_next() {
    // A stale or forged cursor counts as the head: the client gets the run that is open there
    // (a preamble), and then whatever happens next.
    let mut connect = Connect::new(meta(), 99, 3, Follow::Forever);
    assert_eq!(connect.cursor(), 3, "clamped to the head");
    let mut frames = Vec::new();
    for e in [
        user(1, "go"),
        status(2, AgentStatus::Working),
        state(3, ThreadState::Working),
    ] {
        frames.extend(connect.feed(&e));
    }
    let opening = lines(&frames);
    assert_eq!(opening.len(), 3, "{opening:?}");
    assert!(opening[0].starts_with("RUN_STARTED"));
    frames.clear();
    for e in [
        status(4, AgentStatus::Completed),
        state(5, ThreadState::Done),
    ] {
        frames.extend(connect.feed(&e));
    }
    let rest = lines(&frames);
    assert!(rest.last().unwrap().starts_with("RUN_FINISHED"), "{rest:?}");
    assert!(
        !rest.iter().any(|k| k.starts_with("RUN_STARTED")),
        "{rest:?}"
    );
}

#[test]
fn the_preamble_comes_at_the_cursor_even_before_the_next_event() {
    // A client at seq 3 of a run that is going: the stream opens with the run at once, not with
    // the next event that happens to arrive.
    let mut connect = Connect::new(meta(), 3, 3, Follow::Forever);
    let mut frames = Vec::new();
    for e in [
        user(1, "go"),
        status(2, AgentStatus::Working),
        state(3, ThreadState::Working),
    ] {
        frames.extend(connect.feed(&e));
    }
    let kinds = lines(&frames);
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    assert!(kinds[0].starts_with("RUN_STARTED"));
    assert!(kinds[1].starts_with("SUBAGENT_STARTED"));
    assert!(kinds[2].starts_with("STATE_SNAPSHOT"));
    assert!(frames.iter().all(|f| f.resume_id.is_none()));
}

#[test]
fn an_idle_thread_has_no_preamble() {
    let mut connect = Connect::new(meta(), 4, 4, Follow::Forever);
    let mut frames = Vec::new();
    for e in [
        user(1, "go"),
        status(2, AgentStatus::Completed),
        state(3, ThreadState::Done),
    ] {
        frames.extend(connect.feed(&e));
    }
    // The log has three events; the cursor was clamped to 3, and the run is closed there.
    assert!(frames.is_empty(), "{:?}", lines(&frames));
}

#[test]
fn mode_run_on_an_empty_or_idle_thread_is_over_at_once() {
    assert!(Connect::new(meta(), 0, 0, Follow::ThroughRun).finished());
    assert!(!Connect::new(meta(), 0, 0, Follow::Forever).finished());

    // Idle after the replay: finished exactly when the last event of the log is folded.
    let events = [
        user(1, "go"),
        status(2, AgentStatus::Completed),
        state(3, ThreadState::Done),
    ];
    let mut connect = Connect::new(meta(), 0, 3, Follow::ThroughRun);
    for (i, e) in events.iter().enumerate() {
        assert!(!connect.finished(), "before event {i}");
        connect.feed(e);
    }
    assert!(connect.finished());
}

#[test]
fn a_gap_in_the_log_still_starts_the_stream_after_the_cursor() {
    // Sequence numbers are contiguous today; the fold does not depend on it.
    let mut connect = Connect::new(meta(), 3, 5, Follow::Forever);
    let mut frames = Vec::new();
    for e in [
        user(1, "go"),
        status(2, AgentStatus::Working),
        status(5, AgentStatus::Completed),
    ] {
        frames.extend(connect.feed(&e));
    }
    let kinds = lines(&frames);
    assert!(kinds[0].starts_with("RUN_STARTED"), "{kinds:?}");
    assert!(
        kinds.iter().any(|k| k.starts_with("SUBAGENT_FINISHED")),
        "{kinds:?}"
    );
}
