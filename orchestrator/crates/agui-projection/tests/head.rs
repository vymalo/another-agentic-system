//! Where the log ends (`docs/api/agui.md`, "Resume points"): events that have no frame
//! (`thread_shared`, `thread_unshared`, `ui_catalog`, and what a reader's projection makes inert)
//! carry no `id:`, so a stream that replays a log which ends in them would never reach the head the
//! thread's `lastSeq` names. `Connect::head_marker` says, once the replay has caught up, the `seq`
//! to write as an `id:`-only block (no `data:`: a reader that is not told of it reads the stream
//! as it did before).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Connect, Follow, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentMessageData, AgentStatus, AgentStatusData, Event, EventBody, ShareLevel,
    ThreadSharedData, ThreadState, ThreadStateData, ThreadUnsharedData, Timestamp, UiCatalogData,
    UserId, UserMessageData,
};
use proptest::prelude::*;
use serde_json::json;
use support::log::{arb_actions, build, meta, thread_id};

/// What a connect stream writes: a frame, or an `id:` alone.
#[derive(Debug, Clone, PartialEq)]
enum Out {
    Frame(Box<Frame>),
    Head(i64),
}

impl Out {
    /// The `id:` this puts on the wire, if it has one.
    fn id(&self) -> Option<i64> {
        match self {
            Out::Frame(f) => f.resume_id,
            Out::Head(seq) => Some(*seq),
        }
    }
}

/// The loop of the surface, in miniature: fold, write the frames, then the head when it is due.
struct Wire {
    connect: Connect,
}

impl Wire {
    fn new(connect: Connect) -> Self {
        Wire { connect }
    }

    fn feed(&mut self, event: &Event) -> Vec<Out> {
        let mut out: Vec<Out> = self
            .connect
            .feed(event)
            .into_iter()
            .map(|frame| Out::Frame(Box::new(frame)))
            .collect();
        if let Some(seq) = self.connect.head_marker() {
            out.push(Out::Head(seq));
        }
        out
    }

    fn feed_all(&mut self, events: &[Event]) -> Vec<Out> {
        events.iter().flat_map(|e| self.feed(e)).collect()
    }
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
        EventBody::UserMessage(UserMessageData::new("go")),
    )
}

fn status(seq: i64, status: AgentStatus) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: None,
            tier: None,
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

fn shared(seq: i64) -> Event {
    ev(
        seq,
        alice(),
        EventBody::ThreadShared(ThreadSharedData {
            visibility: ShareLevel::Public,
            nonce_sha256: "00".repeat(32),
        }),
    )
}

fn unshared(seq: i64) -> Event {
    ev(
        seq,
        alice(),
        EventBody::ThreadUnshared(ThreadUnsharedData {}),
    )
}

fn catalog(seq: i64) -> Event {
    ev(
        seq,
        alice(),
        EventBody::UiCatalog(UiCatalogData {
            catalog_id: "https://example.com/catalog".into(),
            version: 1,
            digest: format!("sha256:{}", "0".repeat(64)),
            catalog: json!({"catalogId": "https://example.com/catalog", "components": {}}),
        }),
    )
}

/// A finished job: three events, the last of which ends the run.
fn done() -> Vec<Event> {
    vec![
        user(1),
        status(2, AgentStatus::Completed),
        state(3, ThreadState::Done),
    ]
}

fn heads(out: &[Out]) -> Vec<i64> {
    out.iter()
        .filter_map(|o| match o {
            Out::Head(seq) => Some(*seq),
            Out::Frame(_) => None,
        })
        .collect()
}

#[test]
fn a_log_that_ends_in_events_with_no_frame_is_told_where_it_ends() {
    let mut log = done();
    log.extend([shared(4), catalog(5), unshared(6)]);
    let mut wire = Wire::new(Connect::new(meta(), 0, 6, Follow::Forever));
    let out = wire.feed_all(&log);

    assert_eq!(heads(&out), vec![6], "once, at the head");
    assert_eq!(
        out.last(),
        Some(&Out::Head(6)),
        "after the last frame, which is the run's end"
    );
    let last_frame_id = out
        .iter()
        .filter_map(|o| match o {
            Out::Frame(f) => f.resume_id,
            Out::Head(_) => None,
        })
        .next_back();
    assert_eq!(last_frame_id, Some(3), "the frames end at 3, the head is 6");
}

#[test]
fn a_log_that_ends_in_a_frame_needs_no_marker() {
    let mut wire = Wire::new(Connect::new(meta(), 0, 3, Follow::Forever));
    let out = wire.feed_all(&done());
    assert!(heads(&out).is_empty(), "{out:?}");
    assert_eq!(out.last().and_then(Out::id), Some(3));
}

#[test]
fn nothing_is_said_before_the_replay_has_reached_the_head() {
    let mut log = done();
    log.extend([shared(4), shared(5)]);
    // the thread was 5 events long when the client connected
    let mut wire = Wire::new(Connect::new(meta(), 0, 5, Follow::Forever));
    let out = wire.feed_all(&log[..4]);
    assert!(heads(&out).is_empty(), "4 of 5 folded: {out:?}");
    let out = wire.feed(&log[4]);
    assert_eq!(out, vec![Out::Head(5)]);
}

#[test]
fn a_client_that_holds_the_head_is_told_nothing_and_one_behind_it_is_told_the_head() {
    let mut log = done();
    log.extend([shared(4), unshared(5)]);
    // it holds everything: the cursor is the head
    let mut wire = Wire::new(Connect::new(meta(), 5, 5, Follow::Forever));
    assert!(wire.feed_all(&log).is_empty());
    // it holds the run's end (3) and the two events after it have no frame
    let mut wire = Wire::new(Connect::new(meta(), 3, 5, Follow::Forever));
    assert_eq!(wire.feed_all(&log), vec![Out::Head(5)]);
    // a cursor between: the same
    let mut wire = Wire::new(Connect::new(meta(), 4, 5, Follow::Forever));
    assert_eq!(wire.feed_all(&log), vec![Out::Head(5)]);
    // a cursor beyond the log counts as the head (stale or forged): nothing to say
    let mut wire = Wire::new(Connect::new(meta(), 99, 5, Follow::Forever));
    assert!(wire.feed_all(&log).is_empty());
}

#[test]
fn a_thread_with_no_frame_at_all_is_still_told_its_head() {
    // shared, and nothing else: the stream has no frame, and the thread's lastSeq is 1
    let mut wire = Wire::new(Connect::new(meta(), 0, 1, Follow::Forever));
    assert_eq!(wire.feed_all(&[shared(1)]), vec![Out::Head(1)]);
}

#[test]
fn a_frameless_event_that_comes_after_the_replay_is_marked_as_it_comes() {
    let mut wire = Wire::new(Connect::new(meta(), 0, 3, Follow::Forever));
    assert!(heads(&wire.feed_all(&done())).is_empty());
    assert_eq!(wire.feed(&shared(4)), vec![Out::Head(4)]);
    // the next frame carries its own id, and a second frameless event is marked again
    let next = wire.feed(&user(5));
    assert!(heads(&next).is_empty(), "{next:?}");
    assert!(next.last().and_then(Out::id) == Some(5));
    assert_eq!(wire.feed(&catalog(6)), vec![Out::Head(6)]);
}

#[test]
fn a_message_that_is_open_holds_the_marker_back() {
    // the agent is mid-sentence (a partial message) when the owner shares the thread: the head is
    // not a place to resume from, because that would split the message
    let partial = ev(
        3,
        plain(),
        EventBody::AgentMessage(AgentMessageData {
            is_final: false,
            ..AgentMessageData::plain("m1", "so far")
        }),
    );
    let log = vec![user(1), status(2, AgentStatus::Working), partial, shared(4)];
    let mut wire = Wire::new(Connect::new(meta(), 0, 4, Follow::Forever));
    let out = wire.feed_all(&log);
    assert!(heads(&out).is_empty(), "{out:?}");
    // and once the message is whole, its own frame has the id
    let whole = ev(
        5,
        plain(),
        EventBody::AgentMessage(AgentMessageData::plain("m1", "so far, and the rest")),
    );
    let out = wire.feed(&whole);
    assert!(heads(&out).is_empty());
    assert_eq!(out.last().and_then(Out::id), Some(5), "{out:?}");
}

#[test]
fn mode_run_says_the_head_before_it_ends() {
    let mut log = done();
    log.push(shared(4));
    let mut wire = Wire::new(Connect::new(meta(), 0, 4, Follow::ThroughRun));
    let out = wire.feed_all(&log);
    assert_eq!(out.last(), Some(&Out::Head(4)));
    assert!(wire.connect.finished(), "and then the stream is over");
}

#[test]
fn resuming_at_the_head_gives_what_follows_it() {
    let mut log = done();
    log.extend([shared(4), unshared(5)]);
    let mut wire = Wire::new(Connect::new(meta(), 0, 5, Follow::Forever));
    let head = heads(&wire.feed_all(&log))[0];
    assert_eq!(head, 5);

    // a client that reconnects with the head as its cursor is given nothing of the log, and then
    // exactly what an uninterrupted stream would have written after it
    let later = [user(6), status(7, AgentStatus::Completed)];
    let mut reconnect = Wire::new(Connect::new(meta(), head, 5, Follow::Forever));
    assert!(reconnect.feed_all(&log).is_empty());
    let got = reconnect.feed_all(&later);
    let mut uninterrupted = Wire::new(Connect::new(meta(), 0, 5, Follow::Forever));
    uninterrupted.feed_all(&log);
    assert_eq!(got, uninterrupted.feed_all(&later));
}

/// A frameless event of the kinds a log can end in.
fn arb_frameless() -> impl Strategy<Value = u8> {
    0..3_u8
}

fn frameless(kind: u8, seq: i64) -> Event {
    match kind {
        0 => shared(seq),
        1 => unshared(seq),
        _ => catalog(seq),
    }
}

proptest! {
    /// Whatever a legal log is, and however many events with no frame it ends in: the ids a stream
    /// writes only grow, and unless a message of the log is open at its end the last of them is
    /// the log's last `seq`; resuming from that id gives nothing of the log again.
    #[test]
    fn a_stream_reaches_the_head_of_any_log(
        actions in arb_actions(),
        tail in proptest::collection::vec(arb_frameless(), 0..4),
    ) {
        let mut events = build(&actions);
        let mut seq = i64::try_from(events.len()).unwrap();
        for kind in tail {
            seq += 1;
            events.push(frameless(kind, seq));
        }
        let head = seq;

        let mut wire = Wire::new(Connect::new(meta(), 0, head, Follow::Forever));
        let out = wire.feed_all(&events);
        let ids: Vec<i64> = out.iter().filter_map(Out::id).collect();
        prop_assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids only grow: {:?}", ids);

        let mut probe = Projector::new(meta());
        for e in &events {
            probe.apply(e, Audience::Viewer);
        }
        // a message of the log that is open at the end is the one place a stream may not say the head
        if !probe.text_open() && !events.is_empty() {
            prop_assert_eq!(ids.last().copied(), Some(head), "the stream reaches the head");
            // from there, a reconnect has nothing of the log to say but the preamble of a run that is open
            let mut again = Wire::new(Connect::new(meta(), head, head, Follow::Forever));
            let replay = again.feed_all(&events);
            let frames = replay.iter().filter(|o| matches!(o, Out::Frame(_))).count();
            prop_assert_eq!(frames, probe.resume_preamble().len());
            prop_assert!(heads(&replay).is_empty());
        }
    }
}

/// The connect knows what it has handed out: a marker is said once however often it is asked, and
/// again only for a later event that has no frame.
#[test]
fn a_marker_is_said_once_however_often_it_is_asked() {
    let mut log = done();
    log.push(shared(4));
    let mut connect = Connect::new(meta(), 0, 4, Follow::Forever);
    for event in &log {
        let _frames = connect.feed(event);
    }
    assert_eq!(connect.head_marker(), Some(4));
    assert_eq!(connect.head_marker(), None, "said already");
    assert_eq!(connect.head_marker(), None);

    let _frames = connect.feed(&unshared(5));
    assert_eq!(
        connect.head_marker(),
        Some(5),
        "a later event with no frame"
    );
    assert_eq!(connect.head_marker(), None);
}

/// A frame's `id:` counts as written without the caller saying so, and so does the cursor.
#[test]
fn what_the_frames_and_the_cursor_carry_is_not_said_again() {
    // the last event has a frame: its id names the head
    let mut connect = Connect::new(meta(), 0, 3, Follow::Forever);
    let frames: Vec<Frame> = done().iter().flat_map(|e| connect.feed(e)).collect();
    assert_eq!(frames.last().and_then(|f| f.resume_id), Some(3));
    assert_eq!(connect.head_marker(), None);

    // a client that holds the head (its cursor) is not told it
    let mut log = done();
    log.push(shared(4));
    let mut connect = Connect::new(meta(), 4, 4, Follow::Forever);
    for event in &log {
        let _frames = connect.feed(event);
    }
    assert_eq!(connect.head_marker(), None);
}
