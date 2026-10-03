//! Live text (ADR 0027, `docs/api/agui.md`, "Live text"): each rule of the overlay on hand-written
//! logs, then properties over random interleavings of live pieces and log events.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Connect, Follow, Frame, LiveOverlay, Projector};
use orch_agui_proto::Event as Wire;
use orch_core::{
    Actor, AgentId, AgentMessageData, AgentStatus, AgentStatusData, Event, EventBody, LiveChunk,
    LiveEnd, LiveText, MessagePurpose, ThreadState, ThreadStateData, Timestamp, UserId,
    UserMessageData,
};
use support::log::{meta, thread_id};
use support::{line, verify};

// ---- a thread, by hand ------------------------------------------------------------------

fn ev(seq: i64, actor: Actor, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    }
}

fn agent() -> Actor {
    Actor::agent(&AgentId::new("plain"), None)
}

fn user(seq: i64, text: &str) -> Event {
    ev(
        seq,
        Actor::user(&UserId::new("alice@example.com")),
        EventBody::UserMessage(UserMessageData::new(text)),
    )
}

fn status(seq: i64, status: AgentStatus, detail: Option<&str>) -> Event {
    ev(
        seq,
        agent(),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: detail.map(str::to_owned),
        }),
    )
}

fn message(seq: i64, id: &str, text: &str) -> Event {
    ev(
        seq,
        agent(),
        EventBody::AgentMessage(AgentMessageData {
            text: text.to_owned(),
            message_id: id.to_owned(),
            is_final: true,
            purpose: None,
            via: None,
        }),
    )
}

/// A final message the log marked (ADR 0031).
fn message_as(seq: i64, id: &str, text: &str, purpose: MessagePurpose) -> Event {
    ev(
        seq,
        agent(),
        EventBody::AgentMessage(AgentMessageData {
            purpose: Some(purpose),
            ..AgentMessageData::plain(id, text)
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

fn piece(id: &str, offset: u64, text: &str, end: LiveEnd) -> LiveText {
    LiveText {
        thread: thread_id(),
        agent: AgentId::new("plain"),
        chunk: LiveChunk {
            message_id: id.to_owned(),
            offset,
            text: text.to_owned(),
            end,
        },
    }
}

fn open(id: &str, offset: u64, text: &str) -> LiveText {
    piece(id, offset, text, LiveEnd::Open)
}

/// What a frame says, with the live metadata spelled out.
fn show(frame: &Frame) -> String {
    let live = |meta: &Option<orch_agui_proto::Metadata>| {
        meta.as_ref()
            .and_then(|m| m.get("vymalo.live"))
            .map(|v| format!(" live{v}"))
            .unwrap_or_default()
    };
    let id = frame
        .resume_id
        .map(|n| format!("  id:{n}"))
        .unwrap_or_default();
    // what the log said the words are for (ADR 0031), on a START only
    let purpose = |meta: &Option<orch_agui_proto::Metadata>| {
        meta.as_ref()
            .and_then(|m| m.get("vymalo.purpose"))
            .and_then(|v| v.as_str())
            .map(|p| format!(" purpose={p}"))
            .unwrap_or_default()
    };
    match &frame.event {
        Wire::TextMessageStart(e) => format!(
            "TEXT_MESSAGE_START {} {}{}{}{id}",
            e.message_id,
            e.subagent_run_id
                .as_ref()
                .map(|s| format!("@{s}"))
                .unwrap_or_default(),
            live(&e.base.metadata),
            purpose(&e.base.metadata)
        ),
        Wire::TextMessageContent(e) => format!(
            "TEXT_MESSAGE_CONTENT {} {:?}{}{id}",
            e.message_id,
            e.delta,
            live(&e.base.metadata)
        ),
        Wire::TextMessageEnd(e) => format!(
            "TEXT_MESSAGE_END {}{}{id}",
            e.message_id,
            live(&e.base.metadata)
        ),
        _ => line(frame),
    }
}

/// A connection: a projector, its overlay and the model of the reference consumer, every frame
/// checked as it is written.
struct Conn {
    projector: Projector,
    overlay: LiveOverlay,
    checker: verify::Checker,
    written: Vec<Frame>,
}

impl Conn {
    fn new() -> Self {
        Conn {
            projector: Projector::new(meta()),
            overlay: LiveOverlay::new(),
            checker: verify::Checker::new(),
            written: Vec::new(),
        }
    }

    fn write(&mut self, frames: &[Frame]) -> Vec<String> {
        if let Err(e) = self.checker.feed_frames(frames) {
            panic!(
                "{e}\nwritten so far:\n{}\nthis call:\n{}",
                self.written.iter().map(show).collect::<Vec<_>>().join("\n"),
                frames.iter().map(show).collect::<Vec<_>>().join("\n")
            );
        }
        self.written.extend(frames.iter().cloned());
        frames.iter().map(show).collect()
    }

    /// A log event, through the projector and the overlay.
    fn log(&mut self, event: Event) -> Vec<String> {
        let frames = self.projector.apply(&event, Audience::Viewer);
        let frames = self.overlay.logged(&self.projector, frames);
        self.write(&frames)
    }

    /// A piece of live text.
    fn live(&mut self, piece: LiveText) -> Vec<String> {
        let frames = self.overlay.live(&self.projector, &piece);
        for f in &frames {
            assert_eq!(f.resume_id, None, "a live frame is never a resume point");
        }
        self.write(&frames)
    }

    /// The thread as the stories below start: the user speaks, the agent starts working.
    fn working() -> Self {
        let mut c = Conn::new();
        c.log(user(1, "hi"));
        c.log(status(2, AgentStatus::Working, None));
        c
    }
}

fn sent(seq: i64, text: &str, message_id: &str, run_id: &str) -> Event {
    ev(
        seq,
        Actor::user(&UserId::new("alice@example.com")),
        EventBody::UserMessage(UserMessageData {
            text: text.to_owned(),
            message_id: Some(message_id.to_owned()),
            run_id: Some(run_id.to_owned()),
            origin: orch_core::Origin::Agui,
            delivery: Some(orch_core::Delivery::Steer),
            mentions: Vec::new(),
        }),
    )
}

fn is_prefix_of_nothing(frames: &[String]) -> bool {
    frames.is_empty()
}

// ---- rule 1: live pieces -----------------------------------------------------------------

#[test]
fn a_piece_at_offset_zero_opens_a_live_message_and_the_next_ones_grow_it() {
    let mut c = Conn::working();
    assert_eq!(
        c.live(open("S", 0, "Fib")),
        [
            "TEXT_MESSAGE_START S @sub-2 live{}",
            "TEXT_MESSAGE_CONTENT S \"Fib\" live{\"offset\":0}"
        ]
    );
    assert_eq!(
        c.live(open("S", 3, "onacci ")),
        ["TEXT_MESSAGE_CONTENT S \"onacci \" live{\"offset\":3}"]
    );
    assert_eq!(
        c.live(open("S", 10, "in Rust.")),
        ["TEXT_MESSAGE_CONTENT S \"in Rust.\" live{\"offset\":10}"]
    );
    assert_eq!(c.overlay.open_message(), Some("S"));
    assert_eq!(c.checker.reading("S"), Some("Fibonacci in Rust."));
}

#[test]
fn the_start_names_the_agent_and_carries_its_actor_and_the_live_marker() {
    let mut c = Conn::working();
    c.live(open("S", 0, "hi"));
    let start = c
        .written
        .iter()
        .find_map(|f| match &f.event {
            Wire::TextMessageStart(s) if s.message_id.as_str() == "S" => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(start.name.as_deref(), Some("plain"));
    assert_eq!(start.subagent_run_id.as_ref().unwrap().as_str(), "sub-2");
    let meta = start.base.metadata.unwrap();
    assert_eq!(meta["vymalo.actor"]["name"], "plain");
    assert_eq!(meta["vymalo.actor"]["type"], "agent");
    assert_eq!(meta["vymalo.live"], serde_json::json!({}));
}

#[test]
fn offsets_count_utf16_code_units_and_pieces_are_placed_by_utf8_bytes() {
    // "é" is 2 bytes and 1 unit; "🦀" is 4 bytes and 2 units; "中" is 3 bytes and 1 unit.
    let mut c = Conn::working();
    c.live(open("S", 0, "é🦀"));
    assert_eq!(
        c.live(open("S", 6, "中x")),
        ["TEXT_MESSAGE_CONTENT S \"中x\" live{\"offset\":3}"]
    );
    assert_eq!(
        c.live(open("S", 10, "!")),
        ["TEXT_MESSAGE_CONTENT S \"!\" live{\"offset\":5}"]
    );
    assert_eq!(c.checker.reading("S"), Some("é🦀中x!"));
}

#[test]
fn an_overlap_is_trimmed_a_repeat_says_nothing() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fibonacci"));
    // A refresh from the start, longer than what was said: only the new part is said.
    assert_eq!(
        c.live(open("S", 0, "Fibonacci in")),
        ["TEXT_MESSAGE_CONTENT S \" in\" live{\"offset\":9}"]
    );
    // The same piece again, and a piece inside what was said.
    assert!(c.live(open("S", 0, "Fibonacci in")).is_empty());
    assert!(c.live(open("S", 3, "onacci")).is_empty());
    // A piece that starts inside what was said and goes beyond it.
    assert_eq!(
        c.live(open("S", 9, " in Rust")),
        ["TEXT_MESSAGE_CONTENT S \" Rust\" live{\"offset\":12}"]
    );
    assert_eq!(c.checker.reading("S"), Some("Fibonacci in Rust"));
}

#[test]
fn a_gap_is_ignored_until_the_text_is_repeated_from_the_start() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    // Bytes 3..10 were lost: the piece at 10 cannot be placed.
    assert!(c.live(open("S", 10, "in Rust.")).is_empty());
    assert_eq!(c.checker.reading("S"), Some("Fib"));
    // The refresh from offset 0 repairs it.
    assert_eq!(
        c.live(open("S", 0, "Fibonacci in Rust.")),
        ["TEXT_MESSAGE_CONTENT S \"onacci in Rust.\" live{\"offset\":3}"]
    );
}

#[test]
fn a_message_opens_only_at_its_beginning_and_with_something_to_say() {
    let mut c = Conn::working();
    // Joined mid-stream: nothing is said until the text comes round again from the start.
    assert!(is_prefix_of_nothing(&c.live(open("S", 12, "tail"))));
    assert!(is_prefix_of_nothing(&c.live(open("S", 0, ""))));
    assert_eq!(c.overlay.open_message(), None);
    assert_eq!(
        c.live(open("S", 0, "Fibonacci in Rust. tail")),
        [
            "TEXT_MESSAGE_START S @sub-2 live{}",
            "TEXT_MESSAGE_CONTENT S \"Fibonacci in Rust. tail\" live{\"offset\":0}"
        ]
    );
}

#[test]
fn the_last_piece_changes_nothing_but_its_words() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    assert_eq!(
        c.live(piece("S", 3, "onacci", LiveEnd::Last)),
        ["TEXT_MESSAGE_CONTENT S \"onacci\" live{\"offset\":3}"]
    );
    // Still open: the message the log says next closes it.
    assert_eq!(c.overlay.open_message(), Some("S"));
    assert!(c.live(piece("S", 9, "", LiveEnd::Last)).is_empty());
}

#[test]
fn a_message_that_arrives_while_a_reply_is_written_ends_the_draft_as_abandoned_with_the_run() {
    // ADR 0036: the run of the reply ends with the message, so the live message ends with it,
    // before its invocation is suspended, and what the agent writes next is another message
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    assert_eq!(
        c.log(sent(3, "you were wrong", "m-2", "r-2")),
        [
            "TEXT_MESSAGE_END S live{\"abandoned\":true}",
            "SUBAGENT_FINISHED sub-2 suspended[]",
            "STATE_SNAPSHOT working",
            "RUN_FINISHED run-1 success",
            "RUN_STARTED r-2",
            "STATE_SNAPSHOT working",
            "TEXT_MESSAGE_START m-2 ",
            "TEXT_MESSAGE_CONTENT m-2 \"you were wrong\"",
            "TEXT_MESSAGE_END m-2  id:3",
        ]
    );
    assert_eq!(c.overlay.open_message(), None);
    // the rest of the abandoned draft is late: it is ignored, not attributed to the new run
    assert!(c.live(open("S", 3, "onacci")).is_empty());
    // the agent's next words are said in the new run, under the same invocation
    assert_eq!(
        c.log(status(4, AgentStatus::Working, None)),
        [
            "SUBAGENT_STARTED sub-2 plain",
            "ACTIVITY_SNAPSHOT evt-4 vymalo.status {\"status\":\"working\"} @sub-2  id:4",
        ]
    );
    assert_eq!(
        c.live(open("T", 0, "Sorry")),
        [
            "TEXT_MESSAGE_START T @sub-2 live{}",
            "TEXT_MESSAGE_CONTENT T \"Sorry\" live{\"offset\":0}"
        ]
    );
}

#[test]
fn an_abandoned_stream_ends_with_the_flag_and_its_late_pieces_are_ignored() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    assert_eq!(
        c.live(piece("S", 3, "on", LiveEnd::Abandoned)),
        [
            "TEXT_MESSAGE_CONTENT S \"on\" live{\"offset\":3}",
            "TEXT_MESSAGE_END S live{\"abandoned\":true}"
        ]
    );
    assert_eq!(c.overlay.open_message(), None);
    // A refresh that was already on its way does not bring it back.
    assert!(c.live(open("S", 0, "Fibon")).is_empty());
    assert!(c.live(open("S", 0, "Fibonacci")).is_empty());
}

#[test]
fn giving_up_before_anything_was_said_says_nothing() {
    let mut c = Conn::working();
    assert!(c.live(piece("S", 0, "", LiveEnd::Abandoned)).is_empty());
    assert!(c.live(open("S", 0, "late")).is_empty());
    assert_eq!(c.checker.reading("S"), None);
}

#[test]
fn another_stream_opening_ends_the_open_one_as_given_up() {
    let mut c = Conn::working();
    c.live(open("S1", 0, "before the tool call"));
    assert_eq!(
        c.live(open("S2", 0, "after")),
        [
            "TEXT_MESSAGE_END S1 live{\"abandoned\":true}",
            "TEXT_MESSAGE_START S2 @sub-2 live{}",
            "TEXT_MESSAGE_CONTENT S2 \"after\" live{\"offset\":0}"
        ]
    );
    // A piece of S2 from the middle does not end S2 (it is the open one anyway), and a piece of
    // another stream that cannot open does not end it either.
    assert!(c.live(open("S3", 4, "middle")).is_empty());
    assert_eq!(c.overlay.open_message(), Some("S2"));
}

#[test]
fn a_piece_for_a_message_the_log_already_said_is_late() {
    let mut c = Conn::working();
    c.log(message(3, "S", "Done."));
    assert!(c.live(open("S", 0, "Do")).is_empty());
    assert_eq!(c.overlay.open_message(), None);
}

#[test]
fn a_piece_of_another_agent_is_not_this_invocations() {
    let mut c = Conn::working();
    let mut other = open("S", 0, "hello");
    other.agent = AgentId::new("reviewer");
    assert!(c.live(other).is_empty());
    assert_eq!(c.overlay.open_message(), None);
}

#[test]
fn a_piece_that_lands_inside_a_character_is_ignored() {
    let mut c = Conn::working();
    c.live(open("S", 0, "é"));
    // Byte 1 is the middle of "é": the pieces do not agree with each other.
    assert!(c.live(open("S", 1, "é!")).is_empty());
    assert_eq!(c.checker.reading("S"), Some("é"));
}

#[test]
fn a_message_stops_growing_at_its_bound_and_the_final_delivers_the_rest() {
    let mut c = Conn::working();
    let chunk = "x".repeat(64 * 1024);
    let mut sent = 0u64;
    for _ in 0..4 {
        c.live(open("S", sent, &chunk));
        sent += chunk.len() as u64;
    }
    assert_eq!(
        c.checker.reading("S").unwrap().len(),
        orch_agui_projection::MAX_LIVE_MESSAGE_BYTES
    );
    // Nothing past the bound is said, and nothing after it can be placed.
    assert!(c.live(open("S", sent, "y")).is_empty());
    let full = format!(
        "{}tail",
        "x".repeat(orch_agui_projection::MAX_LIVE_MESSAGE_BYTES)
    );
    let frames = c.log(message(3, "S", &full));
    assert_eq!(
        frames[0],
        format!(
            "TEXT_MESSAGE_CONTENT S \"tail\" live{{\"final\":true,\"offset\":{}}}",
            orch_agui_projection::MAX_LIVE_MESSAGE_BYTES
        )
    );
    assert_eq!(c.checker.reading("S"), Some(full.as_str()));
}

// ---- rule 1: before an invocation is open ------------------------------------------------

#[test]
fn pieces_before_the_invocation_are_held_and_said_after_the_event_that_opens_it() {
    let mut c = Conn::new();
    c.log(user(1, "hi"));
    // The run is open (the user spoke), the agent has not reported yet.
    assert!(c.live(open("S", 0, "Fib")).is_empty());
    assert!(c.live(open("S", 3, "onacci")).is_empty());
    let frames = c.log(status(2, AgentStatus::Working, None));
    let shown: Vec<&str> = frames.iter().map(String::as_str).collect();
    // The event's own frames come first; the held pieces follow, with no resume point of their own.
    let start = shown
        .iter()
        .position(|l| l.starts_with("SUBAGENT_STARTED"))
        .unwrap();
    let live_start = shown
        .iter()
        .position(|l| l.starts_with("TEXT_MESSAGE_START S"))
        .unwrap();
    assert!(start < live_start);
    assert_eq!(
        &shown[live_start..],
        [
            "TEXT_MESSAGE_START S @sub-2 live{}",
            "TEXT_MESSAGE_CONTENT S \"Fib\" live{\"offset\":0}",
            "TEXT_MESSAGE_CONTENT S \"onacci\" live{\"offset\":3}"
        ]
    );
    assert!(
        shown[..live_start].iter().any(|l| l.ends_with("id:2")),
        "the event's own last frame keeps its resume point: {shown:?}"
    );
}

#[test]
fn held_pieces_are_dropped_when_the_run_closes() {
    let mut c = Conn::new();
    c.log(user(1, "hi"));
    c.live(open("S", 0, "Fib"));
    c.log(status(2, AgentStatus::Failed, Some("boom")));
    c.log(state(3, ThreadState::Failed));
    // The next job: its invocation must not start with a reply of the one before.
    c.log(user(4, "again"));
    let frames = c.log(status(5, AgentStatus::Working, None));
    assert!(
        frames.iter().all(|l| !l.contains("TEXT_MESSAGE")),
        "{frames:?}"
    );
}

#[test]
fn no_run_open_means_nothing_to_say_it_in_and_nothing_is_kept() {
    let mut c = Conn::new();
    // Before the first event: no run.
    assert!(c.live(open("S", 0, "Fib")).is_empty());
    c.log(user(1, "hi"));
    let frames = c.log(status(2, AgentStatus::Working, None));
    assert!(frames.iter().all(|l| !l.contains("TEXT_MESSAGE")));
}

#[test]
fn at_most_thirty_two_pieces_are_held_and_the_newest_are_kept() {
    let mut c = Conn::new();
    c.log(user(1, "hi"));
    // 40 refreshes of growing text: the first 8 are forgotten, the rest are enough.
    let text = "abcdefghij".repeat(4);
    for n in 1..=40usize {
        c.live(open("S", 0, &text[..n]));
    }
    let frames = c.log(status(2, AgentStatus::Working, None));
    let content: Vec<&String> = frames
        .iter()
        .filter(|l| l.starts_with("TEXT_MESSAGE_CONTENT S"))
        .collect();
    // The oldest held piece that is still there is the 9th (9 characters); the rest extend it.
    assert_eq!(
        c.checker.reading("S"),
        Some(text.as_str()),
        "{content:?} / {frames:?}"
    );
}

// ---- rule 2: the log's final message completes the live one ------------------------------

#[test]
fn the_final_message_continues_the_live_one_and_keeps_its_resume_point() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    c.live(open("S", 3, "onacci "));
    c.live(open("S", 10, "in Rust."));
    assert_eq!(
        c.log(message(3, "S", "Fibonacci in Rust.")),
        [
            "TEXT_MESSAGE_CONTENT S \"\" live{\"final\":true,\"offset\":18}",
            "TEXT_MESSAGE_END S live{\"final\":true}  id:3"
        ]
    );
    assert_eq!(c.overlay.open_message(), None);
    assert_eq!(c.checker.reading("S"), Some("Fibonacci in Rust."));
    // The words said again by the status are not said twice (the projector's own rule).
    let frames = c.log(status(
        4,
        AgentStatus::Completed,
        Some("Fibonacci in Rust."),
    ));
    assert!(frames.iter().all(|l| !l.starts_with("TEXT_MESSAGE_START")));
}

#[test]
fn the_final_message_says_only_what_the_viewer_has_not_seen() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    assert_eq!(
        c.log(message(3, "S", "Fibonacci in Rust.")),
        [
            "TEXT_MESSAGE_CONTENT S \"onacci in Rust.\" live{\"final\":true,\"offset\":3}",
            "TEXT_MESSAGE_END S live{\"final\":true}  id:3"
        ]
    );
    assert_eq!(c.checker.reading("S"), Some("Fibonacci in Rust."));
}

#[test]
fn a_final_that_does_not_start_with_what_was_said_replaces_it() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fibonacci"));
    assert_eq!(
        c.log(message(3, "S", "Lucas numbers")),
        [
            "TEXT_MESSAGE_CONTENT S \"Lucas numbers\" live{\"final\":true,\"offset\":0}",
            "TEXT_MESSAGE_END S live{\"final\":true}  id:3"
        ]
    );
    assert_eq!(c.checker.reading("S"), Some("Lucas numbers"));
}

// ---- working text (ADR 0031) ---------------------------------------------------------------

#[test]
fn a_live_stream_the_log_marks_working_ends_as_working() {
    let mut c = Conn::working();
    c.live(open("W", 0, "Let me run "));
    c.live(open("W", 11, "the tests first."));
    // The live message opened before anyone knew what its words are for; the log's message
    // continues it, and its END says it was working text. The START the log wrote is dropped.
    assert_eq!(
        c.log(message_as(
            3,
            "W",
            "Let me run the tests first.",
            MessagePurpose::Working
        )),
        [
            "TEXT_MESSAGE_CONTENT W \"\" live{\"final\":true,\"offset\":27}",
            "TEXT_MESSAGE_END W live{\"final\":true,\"purpose\":\"working\"}  id:3"
        ]
    );
    assert_eq!(c.overlay.open_message(), None);
    assert_eq!(c.checker.reading("W"), Some("Let me run the tests first."));
}

#[test]
fn a_live_stream_the_log_marks_answer_ends_as_final_and_no_more() {
    let mut c = Conn::working();
    c.live(open("R", 0, "Fib"));
    assert_eq!(
        c.log(message_as(3, "R", "Fibonacci.", MessagePurpose::Answer)),
        [
            "TEXT_MESSAGE_CONTENT R \"onacci.\" live{\"final\":true,\"offset\":3}",
            "TEXT_MESSAGE_END R live{\"final\":true}  id:3"
        ]
    );
}

#[test]
fn a_live_stream_the_log_leaves_unmarked_ends_as_it_always_did() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    assert_eq!(
        c.log(message(3, "S", "Fibonacci.")),
        [
            "TEXT_MESSAGE_CONTENT S \"onacci.\" live{\"final\":true,\"offset\":3}",
            "TEXT_MESSAGE_END S live{\"final\":true}  id:3"
        ]
    );
}

#[test]
fn working_text_with_no_live_message_says_it_on_its_own_start() {
    // The log's message alone (a replay, a viewer that joined late): the START carries the
    // purpose, and the overlay has nothing to add.
    let mut c = Conn::working();
    assert_eq!(
        c.log(message_as(3, "W", "Let me look.", MessagePurpose::Working)),
        [
            "TEXT_MESSAGE_START W @sub-2 purpose=working",
            "TEXT_MESSAGE_CONTENT W \"Let me look.\"",
            "TEXT_MESSAGE_END W  id:3"
        ]
    );
}

#[test]
fn working_text_for_a_stream_that_was_given_up_is_said_again_with_its_purpose() {
    let mut c = Conn::working();
    c.live(open("W", 0, "Let me"));
    c.live(piece("W", 6, "", LiveEnd::Abandoned));
    // said under another id (an id is never reused on a stream), still marked
    assert_eq!(
        c.log(message_as(3, "W", "Let me look.", MessagePurpose::Working)),
        [
            "TEXT_MESSAGE_START W~final @sub-2 purpose=working",
            "TEXT_MESSAGE_CONTENT W~final \"Let me look.\"",
            "TEXT_MESSAGE_END W~final  id:3"
        ]
    );
}

#[test]
fn a_final_with_no_live_message_is_the_plain_triad() {
    let mut c = Conn::working();
    let shown = c.log(message(3, "S", "Done."));
    assert_eq!(
        shown,
        [
            "TEXT_MESSAGE_START S @sub-2",
            "TEXT_MESSAGE_CONTENT S \"Done.\"",
            "TEXT_MESSAGE_END S  id:3"
        ]
    );
}

#[test]
fn a_final_for_another_message_leaves_the_open_live_one_alone() {
    let mut c = Conn::working();
    c.live(open("S2", 0, "second"));
    let shown = c.log(message(3, "S1", "first"));
    assert_eq!(
        shown,
        [
            "TEXT_MESSAGE_START S1 @sub-2",
            "TEXT_MESSAGE_CONTENT S1 \"first\"",
            "TEXT_MESSAGE_END S1  id:3"
        ]
    );
    assert_eq!(c.overlay.open_message(), Some("S2"));
}

#[test]
fn a_logged_event_while_a_live_message_is_open_is_still_a_resume_point() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    // A step or a status of the log: its last frame keeps the `id:` (the live message is not in
    // the log, a reconnect from here is told the text again by the refresh).
    let shown = c.log(status(3, AgentStatus::Working, Some("running tests")));
    assert!(shown.last().unwrap().ends_with("id:3"), "{shown:?}");
    assert!(c.checker.live_open());
    assert!(!c.checker.text_open());
}

// ---- rule 2: closing --------------------------------------------------------------------

#[test]
fn an_open_live_message_ends_given_up_before_its_invocation_does() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    // The agent completes without the log ever saying S.
    let shown = c.log(status(3, AgentStatus::Completed, None));
    let end = shown
        .iter()
        .position(|l| l == "TEXT_MESSAGE_END S live{\"abandoned\":true}")
        .unwrap_or_else(|| panic!("{shown:?}"));
    let finished = shown
        .iter()
        .position(|l| l.starts_with("SUBAGENT_FINISHED sub-2"))
        .unwrap();
    assert!(end < finished, "{shown:?}");
    assert_eq!(c.overlay.open_message(), None);
    c.log(state(4, ThreadState::Done));
}

#[test]
fn an_open_live_message_ends_given_up_before_the_run_does() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    let shown = c.log(status(3, AgentStatus::Failed, Some("boom")));
    assert!(
        shown.contains(&"TEXT_MESSAGE_END S live{\"abandoned\":true}".to_owned()),
        "{shown:?}"
    );
    c.log(state(4, ThreadState::Failed));
    assert!(!c.checker.run_open());
}

#[test]
fn a_question_suspends_the_invocation_and_ends_the_live_message_first() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Which"));
    let shown = c.log(status(3, AgentStatus::InputRequired, Some("Which branch?")));
    let end = shown
        .iter()
        .position(|l| l == "TEXT_MESSAGE_END S live{\"abandoned\":true}")
        .unwrap();
    let suspended = shown
        .iter()
        .position(|l| l.starts_with("SUBAGENT_FINISHED sub-2 suspended"))
        .unwrap();
    assert!(end < suspended);
    c.log(state(4, ThreadState::Blocked));
}

#[test]
fn the_message_the_log_says_after_an_abandoned_one_gets_another_id() {
    let mut c = Conn::working();
    c.live(open("S", 0, "Fib"));
    c.live(piece("S", 3, "", LiveEnd::Abandoned));
    // The agent persisted it after all: S was used on the wire, so the final is said under
    // another id (a message id is never reused on a stream).
    assert_eq!(
        c.log(message(3, "S", "Fibonacci")),
        [
            "TEXT_MESSAGE_START S~final @sub-2",
            "TEXT_MESSAGE_CONTENT S~final \"Fibonacci\"",
            "TEXT_MESSAGE_END S~final  id:3"
        ]
    );
    assert_eq!(c.checker.reading("S~final"), Some("Fibonacci"));
}

// ---- the overlay does not change the fold -----------------------------------------------

#[test]
fn the_projector_never_hears_of_live_text() {
    let events = [
        user(1, "hi"),
        status(2, AgentStatus::Working, None),
        message(3, "S", "Fibonacci in Rust."),
        status(4, AgentStatus::Completed, Some("Fibonacci in Rust.")),
        state(5, ThreadState::Done),
    ];
    let mut plain = Projector::new(meta());
    let plain_frames: Vec<Frame> = events
        .iter()
        .flat_map(|e| plain.apply(e, Audience::Viewer))
        .collect();

    let mut c = Conn::new();
    for event in &events[..2] {
        c.log(event.clone());
    }
    c.live(open("S", 0, "Fib"));
    for event in &events[2..] {
        c.log(event.clone());
    }
    // What the overlay wrote, minus its own frames and merged with the final, is the plain stream:
    // the resume points are the same ones.
    let plain_ids: Vec<i64> = plain_frames.iter().filter_map(|f| f.resume_id).collect();
    let live_ids: Vec<i64> = c.written.iter().filter_map(|f| f.resume_id).collect();
    assert_eq!(plain_ids, live_ids);
    assert_eq!(plain.thread_state(), c.projector.thread_state());
    assert_eq!(c.checker.reading("S"), Some("Fibonacci in Rust."));
}

// ---- a connection that starts in the middle --------------------------------------------

#[test]
fn a_connection_that_joins_mid_stream_is_told_the_text_so_far_by_the_refresh() {
    let events = [user(1, "hi"), status(2, AgentStatus::Working, None)];
    // The client holds the log up to seq 2 and reconnects: the preamble re-opens the run.
    let mut connect = Connect::new(meta(), 2, 2, Follow::Forever);
    let mut overlay = LiveOverlay::new();
    let mut checker = verify::Checker::new();
    for e in &events {
        let frames = connect.feed(e);
        let frames = overlay.logged(connect.projector(), frames);
        checker.feed_frames(&frames).unwrap();
    }
    // A piece from the middle of the stream: nothing yet.
    let frames = overlay.live(connect.projector(), &open("S", 3, "onacci"));
    assert!(frames.is_empty());
    // The refresh, from the start: the whole text so far.
    let frames = overlay.live(connect.projector(), &open("S", 0, "Fibonacci"));
    checker.feed_frames(&frames).unwrap();
    assert_eq!(checker.reading("S"), Some("Fibonacci"));
    // And the log's final completes it.
    let frames = connect.feed(&message(3, "S", "Fibonacci in Rust."));
    let frames = overlay.logged(connect.projector(), frames);
    checker.feed_frames(&frames).unwrap();
    assert_eq!(checker.reading("S"), Some("Fibonacci in Rust."));
}

// ---- properties: random interleavings of live pieces and log events ---------------------

mod interleaved {
    use super::*;
    use proptest::prelude::*;

    /// Something that happens to a stream of text: the pieces a relay and a network can make of it.
    #[derive(Debug, Clone)]
    enum Op {
        /// The next `len` characters.
        Next { len: usize },
        /// The last piece sent, again.
        Repeat,
        /// `back` characters before what was sent, and `len` after.
        Overlap { back: usize, len: usize },
        /// Skips `skip` characters of the text and sends `len` after them.
        Gap { skip: usize, len: usize },
        /// What is known from offset 0, the one-second refresh.
        Refresh,
        /// All that is left, as the last piece.
        Last,
        /// Giving up.
        Abandon,
        /// The whole text from offset 0, whenever it arrives.
        Late,
        /// The next characters, from an agent that is not working here.
        Other { len: usize },
    }

    fn arb_op() -> impl Strategy<Value = Op> {
        prop_oneof![
            6 => (1usize..12).prop_map(|len| Op::Next { len }),
            1 => Just(Op::Repeat),
            2 => (0usize..6, 1usize..12).prop_map(|(back, len)| Op::Overlap { back, len }),
            2 => (1usize..8, 1usize..10).prop_map(|(skip, len)| Op::Gap { skip, len }),
            2 => Just(Op::Refresh),
            1 => Just(Op::Last),
            1 => Just(Op::Abandon),
            1 => Just(Op::Late),
            1 => (1usize..8).prop_map(|len| Op::Other { len }),
        ]
    }

    /// The part of `text` between character `from` and character `to`, with its byte offset.
    fn slice(text: &str, from: usize, to: usize) -> (u64, String) {
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let from = from.min(chars.len());
        let to = to.clamp(from, chars.len());
        let start = chars.get(from).map_or(text.len(), |(b, _)| *b);
        let end = chars.get(to).map_or(text.len(), |(b, _)| *b);
        (start as u64, text[start..end].to_owned())
    }

    /// The texts with one more: a reply the agent streams and the log never says (it gave up, or
    /// the turn ended first).
    fn with_ghost(texts: &[String]) -> Vec<String> {
        let mut all = texts.to_vec();
        all.push("a reply nobody persisted".to_owned());
        all
    }

    #[derive(Debug, Clone)]
    enum Item {
        Log(Event),
        Live(LiveText),
    }

    /// The log: the user speaks, the agent works, says each text (a noise event between them),
    /// completes, and the thread is done.
    fn log_of(texts: &[String], noise: bool) -> Vec<Event> {
        let mut events = vec![user(1, "go"), status(2, AgentStatus::Working, None)];
        let mut seq = 3;
        for (i, text) in texts.iter().enumerate() {
            if noise {
                events.push(status(seq, AgentStatus::Working, Some("noise")));
                seq += 1;
            }
            events.push(message(seq, &format!("S{i}"), text));
            seq += 1;
        }
        events.push(status(seq, AgentStatus::Completed, None));
        events.push(state(seq + 1, ThreadState::Done));
        events
    }

    /// The pieces the ops make of the texts, each with the position among the log's events it
    /// arrives before (`pos`).
    fn items_of(events: &[Event], texts: &[String], ops: &[(usize, usize, Op)]) -> Vec<Item> {
        let mut known = vec![0usize; texts.len()]; // characters sent so far, by text
        let mut last: Vec<Option<LiveText>> = vec![None; texts.len()];
        let mut made: Vec<(usize, usize, LiveText)> = Vec::new(); // (position, order, piece)
        for (order, (which, pos, op)) in ops.iter().enumerate() {
            let i = which % texts.len();
            let text = &texts[i];
            let id = format!("S{i}");
            let chars = text.chars().count();
            let mut agent_ok = true;
            let piece = match op {
                Op::Next { len } => {
                    let (offset, part) = slice(text, known[i], known[i] + len);
                    known[i] = (known[i] + len).min(chars);
                    Some(piece(&id, offset, &part, LiveEnd::Open))
                }
                Op::Repeat => last[i].clone(),
                Op::Overlap { back, len } => {
                    let from = known[i].saturating_sub(*back);
                    let (offset, part) = slice(text, from, known[i] + len);
                    known[i] = (known[i] + len).min(chars);
                    Some(piece(&id, offset, &part, LiveEnd::Open))
                }
                Op::Gap { skip, len } => {
                    let (offset, part) = slice(text, known[i] + skip, known[i] + skip + len);
                    known[i] = (known[i] + skip + len).min(chars);
                    Some(piece(&id, offset, &part, LiveEnd::Open))
                }
                Op::Refresh => {
                    let (offset, part) = slice(text, 0, known[i]);
                    Some(piece(&id, offset, &part, LiveEnd::Open))
                }
                Op::Last => {
                    let (offset, part) = slice(text, known[i], chars);
                    known[i] = chars;
                    Some(piece(&id, offset, &part, LiveEnd::Last))
                }
                Op::Abandon => Some(piece(&id, 0, "", LiveEnd::Abandoned)),
                Op::Late => {
                    let (offset, part) = slice(text, 0, chars);
                    Some(piece(&id, offset, &part, LiveEnd::Open))
                }
                Op::Other { len } => {
                    agent_ok = false;
                    let (offset, part) = slice(text, known[i], known[i] + len);
                    Some(piece(&id, offset, &part, LiveEnd::Open))
                }
            };
            let Some(mut p) = piece else { continue };
            if agent_ok {
                last[i] = Some(p.clone());
            } else {
                p.agent = AgentId::new("somebody-else");
            }
            made.push((pos % (events.len() + 1), order, p));
        }
        made.sort_by_key(|(pos, order, _)| (*pos, *order));
        let mut items = Vec::new();
        let mut next = made.into_iter().peekable();
        for (index, event) in events.iter().enumerate() {
            while next.peek().is_some_and(|(pos, _, _)| *pos == index) {
                items.push(Item::Live(next.next().unwrap().2));
            }
            items.push(Item::Log(event.clone()));
        }
        items.extend(next.map(|(_, _, p)| Item::Live(p)));
        items
    }

    fn feed(conn: &mut Conn, item: &Item) {
        match item {
            Item::Log(e) => {
                conn.log(e.clone());
            }
            Item::Live(p) => {
                conn.live(p.clone());
            }
        }
    }

    /// The texts of the replies, whether the log has noise between them, the pieces' ops (which
    /// text, where they arrive, what they are), and a seed for choosing where to reconnect.
    type World = (Vec<String>, bool, Vec<(usize, usize, Op)>, usize);

    fn arb_world() -> impl Strategy<Value = World> {
        (
            proptest::collection::vec("[a-zé中🦀 .\n]{1,40}", 1..4),
            any::<bool>(),
            proptest::collection::vec((0usize..4, 0usize..16, arb_op()), 0..30),
            any::<usize>(),
        )
    }

    proptest! {
        /// Every prefix of what is written is well formed (the reference consumer's rules, plus
        /// the offsets of live text), whatever the pieces do: overlap, repeat, skip, arrive late,
        /// before the run, from another agent, or give up. At the end each reply the log said is
        /// read as its text, and nothing is left open.
        #[test]
        fn the_stream_is_well_formed_and_every_reply_reads_as_its_text(
            (texts, noise, ops, _) in arb_world()
        ) {
            let events = log_of(&texts, noise);
            let items = items_of(&events, &with_ghost(&texts), &ops);
            let mut conn = Conn::new();
            for item in &items {
                feed(&mut conn, item);
            }
            prop_assert!(!conn.checker.run_open());
            prop_assert!(!conn.checker.live_open());
            // The reply the log never says is at most what its pieces said, and ended.
            let ghost = format!("S{}", texts.len());
            for (id, said) in conn.checker.readings() {
                if id == &ghost {
                    prop_assert!(with_ghost(&texts)[texts.len()].starts_with(said.as_str()));
                }
            }
            for (i, text) in texts.iter().enumerate() {
                let prefix = format!("S{i}");
                prop_assert!(
                    conn.checker
                        .readings()
                        .iter()
                        .any(|(id, said)| id.starts_with(&prefix) && said == text),
                    "S{i} = {:?} is not read in {:?}",
                    text,
                    conn.checker.readings()
                );
            }
        }

        /// The live frames are never resume points, and the resume points are exactly the plain
        /// projection's: the overlay changes nothing of what a resume means.
        #[test]
        fn the_resume_points_are_the_projections_own((texts, noise, ops, _) in arb_world()) {
            let events = log_of(&texts, noise);
            let items = items_of(&events, &with_ghost(&texts), &ops);
            let mut conn = Conn::new();
            for item in &items {
                feed(&mut conn, item);
            }
            let mut plain = Projector::new(meta());
            let want: Vec<i64> = events
                .iter()
                .flat_map(|e| plain.apply(e, Audience::Viewer))
                .filter_map(|f| f.resume_id)
                .collect();
            let got: Vec<i64> = conn.written.iter().filter_map(|f| f.resume_id).collect();
            prop_assert_eq!(got, want);
        }

        /// A connection that opens at any resume point of another, with nothing but the pieces
        /// that come after, writes a well formed stream, and reads every reply the log says after
        /// its cursor as the reply's text (and none that the cursor already held).
        #[test]
        fn a_new_overlay_started_at_any_resume_point_is_well_formed(
            (texts, noise, ops, seed) in arb_world()
        ) {
            let events = log_of(&texts, noise);
            let items = items_of(&events, &with_ghost(&texts), &ops);
            let k = seed % (items.len() + 1);

            let mut first = Conn::new();
            for item in &items[..k] {
                feed(&mut first, item);
            }
            let cursor = first.written.iter().filter_map(|f| f.resume_id).next_back().unwrap_or(0);
            let head = items[..k]
                .iter()
                .filter_map(|i| match i {
                    Item::Log(e) => Some(e.seq),
                    Item::Live(_) => None,
                })
                .max()
                .unwrap_or(0);

            let mut connect = Connect::new(meta(), cursor, head, Follow::Forever);
            let mut overlay = LiveOverlay::new();
            let mut checker = verify::Checker::new();
            let write = |frames: Vec<Frame>, checker: &mut verify::Checker| {
                checker.feed_frames(&frames).map_err(TestCaseError::fail)
            };
            for e in events.iter().filter(|e| e.seq <= head) {
                let frames = connect.feed(e);
                let frames = overlay.logged(connect.projector(), frames);
                write(frames, &mut checker)?;
            }
            for item in &items[k..] {
                let frames = match item {
                    Item::Log(e) => {
                        let frames = connect.feed(e);
                        overlay.logged(connect.projector(), frames)
                    }
                    Item::Live(p) => overlay.live(connect.projector(), p),
                };
                write(frames, &mut checker)?;
            }
            prop_assert!(!checker.run_open());
            prop_assert!(!checker.live_open());
            for (i, text) in texts.iter().enumerate() {
                let prefix = format!("S{i}");
                let seq = events
                    .iter()
                    .find_map(|e| match &e.body {
                        EventBody::AgentMessage(m) if m.message_id == prefix => Some(e.seq),
                        _ => None,
                    })
                    .unwrap();
                let read = checker
                    .readings()
                    .iter()
                    .any(|(id, said)| id.starts_with(&prefix) && said == text);
                if seq > cursor {
                    prop_assert!(read, "S{i} was not read after the cursor {cursor}");
                } else {
                    prop_assert!(
                        !checker.readings().keys().any(|id| id.starts_with(&prefix)),
                        "S{i} is held by the cursor {cursor} and was said again"
                    );
                }
            }
        }
    }
}
