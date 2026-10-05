//! Live text on an AG-UI stream (ADR 0027, `docs/api/agui.md` "Live text").
//!
//! The log holds an agent's reply once, when it is final. While the agent is still writing it,
//! the words travel as [`LiveText`] pieces (relayed between processes, never stored), and a
//! [`LiveOverlay`] turns them into frames **beside** the projection:
//!
//! ```text
//!   log event --> Projector::apply --> frames --> LiveOverlay::logged --> written, with `id:`
//!   live piece ------------------------------> LiveOverlay::live   --> written, never an `id:`
//! ```
//!
//! The overlay never changes the projector's fold, so a replay, a reconnect and an export see
//! exactly what they saw before; it belongs to one connection and is rebuilt (empty) by the next
//! one, which is why a live frame is not a resume point. The rules:
//!
//! 1. A piece for a message the log has already said, or one closed a moment ago, is late and is
//!    ignored. With no invocation open to attribute it to it is held (a few pieces, dropped when
//!    the run closes) until the next logged event opens one. A message opens at a piece that
//!    starts at offset 0 (`TEXT_MESSAGE_START` with `vymalo.live` metadata, then `CONTENT`), and
//!    grows by the part of each piece beyond what was already said: an overlap is trimmed, a gap
//!    is ignored until the sender repeats the text from the start. Every live `CONTENT` says, in
//!    `metadata["vymalo.live"].offset`, how many UTF-16 code units were said before it. A stream
//!    that is given up ends with `END` and `abandoned`; so does one when another stream opens, and
//!    one still open when its invocation or the run closes.
//! 2. When the log's final message for the open id arrives, it **continues** the live message:
//!    its `START` is dropped, its `CONTENT` carries only the words not yet said, with the offset
//!    they continue from and `final: true`, and its `END` closes the same message and keeps the
//!    resume point. A final that does not start with what was said replaces it (offset 0, the
//!    whole text). When the log marked that message **working text** (`purpose`, ADR 0031), the
//!    `END` says it too, `{final: true, purpose: "working"}`: the live message opened before
//!    anyone knew what its words were for, and this is where the screen learns that they were
//!    not the answer. An answer's `END` says nothing more than `final`.
//! 3. Live frames never carry a resume id, and neither do they hold one back: a live message is
//!    not in the log, so a frame of the log written while one is open is still a resume point.
//!
//! 4. **Reasoning** (ADR 0044, [`LiveKind::Reasoning`]) is a second lane with the same rules, in
//!    AG-UI's reasoning events: a piece at offset 0 opens `REASONING_START` + `REASONING_MESSAGE_START`
//!    (both with `vymalo.live` metadata) and then `REASONING_MESSAGE_CONTENT` grows it; the log's
//!    `agent_reasoning` (its five events, in one group) **continues** it: the two `START`s are dropped,
//!    the `CONTENT` carries the words not yet said with `{offset, final: true}`, and the two ends keep
//!    `{final: true}` (the last one the resume point). A stream given up, or still open when its
//!    invocation or the run closes, ends with `abandoned`; the log's reasoning for an id that was given
//!    up is said under `<id>~final`. A reasoning lane never touches the text lane: the two are open at
//!    once when the log's reasoning has not yet arrived and the words have begun.
//!
//! Hand the overlay live text only once the log has been folded up to what it held when the
//! connection opened: a piece that arrives during the replay of old events would be attributed
//! to whatever invocation that replay has open.

use std::collections::VecDeque;

use orch_agui_proto::{
    self as agui, Metadata, ReasoningEndEvent, ReasoningMessageContentEvent,
    ReasoningMessageEndEvent, ReasoningMessageStartEvent, ReasoningStartEvent, SubagentRunId,
    TextMessageContentEvent, TextMessageEndEvent, TextMessageRole, TextMessageStartEvent,
};
use orch_core::{LiveEnd, LiveKind, LiveText, MessagePurpose};
use serde_json::{Value, json};

use crate::frame::Frame;
use crate::projector::Projector;
use crate::vocab::{LIVE_KEY, PURPOSE_KEY, actor_metadata};

/// The most text one live message holds, in bytes. A stream that goes on past it stops growing on
/// the screen (the pieces that would not fit are ignored, and so is anything after them until the
/// text is repeated from the start, which it never is past the limit), and the final message
/// delivers the rest in one `CONTENT`.
pub const MAX_LIVE_MESSAGE_BYTES: usize = 256 * 1024;
/// How many pieces are held while no invocation is open.
const HELD_MAX: usize = 32;
/// How many closed message ids are remembered, to ignore the pieces that arrive late.
const CLOSED_MEMORY: usize = 64;

/// The live message that is open on the wire.
#[derive(Debug, Clone)]
struct OpenLive {
    /// The message id on the wire: the id of the stream.
    id: String,
    /// The invocation it is attributed to.
    sub: SubagentRunId,
    /// What was said so far.
    sent: String,
}

/// How a live message ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Closed {
    /// The log's final message took it over.
    Merged,
    /// It was given up: its id was used on the wire and is gone.
    Abandoned,
}

/// What the frames of one logged event say about a message whose id the overlay already used.
#[derive(Debug, Clone)]
enum Rewrite {
    /// The log's final message for the open live message: `CONTENT` and `END` continue it.
    /// `working` says that the log marked it working text (ADR 0031): its `END` says so too.
    Merge {
        id: String,
        sent: String,
        working: bool,
    },
    /// The final message for an id that was given up on the wire: said again under another id.
    Rename { from: String, to: String },
}

/// Live text for one connection. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct LiveOverlay {
    open: Option<OpenLive>,
    /// The live reasoning that is open on the wire (ADR 0044).
    reasoning: Option<OpenLive>,
    closed: VecDeque<(String, Closed)>,
    held: Vec<LiveText>,
}

/// What the frames of one logged `agent_reasoning` say about a reasoning whose id the overlay already used.
#[derive(Debug, Clone)]
enum ReasoningRewrite {
    /// The log's reasoning for the open live one: its two `START`s are dropped, its `CONTENT` and ends continue it.
    Merge { id: String, sent: String },
    /// The log's reasoning for an id that was given up on the wire: said again under another id.
    Rename { from: String, to: String },
}

fn live_metadata(value: Value) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert(LIVE_KEY.to_owned(), value);
    metadata
}

fn plain(event: impl Into<agui::Event>) -> Frame {
    Frame {
        event: event.into(),
        resume_id: None,
    }
}

/// UTF-16 code units: the length the client's strings count in.
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

impl LiveOverlay {
    /// An overlay with nothing open.
    pub fn new() -> Self {
        Self::default()
    }

    /// The id of the live message that is open, if any.
    pub fn open_message(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.id.as_str())
    }

    /// The id of the live reasoning that is open, if any.
    pub fn open_reasoning(&self) -> Option<&str> {
        self.reasoning.as_ref().map(|o| o.id.as_str())
    }

    /// Passes the frames `projector` produced for one log event (call it **after**
    /// [`Projector::apply`], with the projector as it is then) and returns what to write: the same
    /// frames, except that the final message of an open live message continues it, and that an
    /// open live message ends (given up) before the frame that closes its invocation or the run.
    /// Held pieces are said after the frames if the event opened an invocation.
    pub fn logged(&mut self, projector: &Projector, frames: Vec<Frame>) -> Vec<Frame> {
        let mut out: Vec<Frame> = Vec::with_capacity(frames.len());
        let mut rewrite: Option<Rewrite> = None;
        let mut reasoning_rewrite: Option<ReasoningRewrite> = None;
        for mut frame in frames {
            match &mut frame.event {
                agui::Event::ReasoningStart(start) => {
                    let id = start.message_id.as_str().to_owned();
                    if self.reasoning.as_ref().is_some_and(|o| o.id == id) {
                        if let Some(open) = self.reasoning.take() {
                            self.remember(&id, Closed::Merged);
                            reasoning_rewrite = Some(ReasoningRewrite::Merge {
                                id,
                                sent: open.sent,
                            });
                        }
                        // The live reasoning is already open on the wire: this START is dropped.
                        continue;
                    }
                    if self.was(&id, Closed::Abandoned) {
                        let to = format!("{id}~final");
                        start.message_id = agui::MessageId::new(to.clone());
                        reasoning_rewrite = Some(ReasoningRewrite::Rename { from: id, to });
                    }
                }
                agui::Event::ReasoningMessageStart(start) => match &reasoning_rewrite {
                    Some(ReasoningRewrite::Merge { id, .. }) if start.message_id.as_str() == id => {
                        continue;
                    }
                    Some(ReasoningRewrite::Rename { from, to })
                        if start.message_id.as_str() == from =>
                    {
                        start.message_id = agui::MessageId::new(to.clone());
                    }
                    _ => {}
                },
                agui::Event::ReasoningMessageContent(content) => match &reasoning_rewrite {
                    Some(ReasoningRewrite::Merge { id, sent })
                        if content.message_id.as_str() == id =>
                    {
                        let rest = content.delta.strip_prefix(sent.as_str()).map(str::to_owned);
                        let (offset, delta) = match rest {
                            Some(rest) => (utf16_len(sent), rest),
                            None => (0, std::mem::take(&mut content.delta)),
                        };
                        content.delta = delta;
                        content.base.metadata =
                            Some(live_metadata(json!({"offset": offset, "final": true})));
                    }
                    Some(ReasoningRewrite::Rename { from, to })
                        if content.message_id.as_str() == from =>
                    {
                        content.message_id = agui::MessageId::new(to.clone());
                    }
                    _ => {}
                },
                agui::Event::ReasoningMessageEnd(end) => match &reasoning_rewrite {
                    Some(ReasoningRewrite::Merge { id, .. }) if end.message_id.as_str() == id => {
                        end.base.metadata = Some(live_metadata(json!({"final": true})));
                    }
                    Some(ReasoningRewrite::Rename { from, to })
                        if end.message_id.as_str() == from =>
                    {
                        end.message_id = agui::MessageId::new(to.clone());
                    }
                    _ => {}
                },
                agui::Event::ReasoningEnd(end) => match &reasoning_rewrite {
                    Some(ReasoningRewrite::Merge { id, .. }) if end.message_id.as_str() == id => {
                        end.base.metadata = Some(live_metadata(json!({"final": true})));
                        reasoning_rewrite = None;
                    }
                    Some(ReasoningRewrite::Rename { from, to })
                        if end.message_id.as_str() == from =>
                    {
                        end.message_id = agui::MessageId::new(to.clone());
                        reasoning_rewrite = None;
                    }
                    _ => {}
                },
                agui::Event::TextMessageStart(start) => {
                    let id = start.message_id.as_str().to_owned();
                    if self.open.as_ref().is_some_and(|o| o.id == id) {
                        if let Some(open) = self.open.take() {
                            self.remember(&id, Closed::Merged);
                            // The START the log wrote carries what the words are for; the live
                            // one could not know, so the END says it (working text only).
                            let working = start
                                .base
                                .metadata
                                .as_ref()
                                .and_then(|m| m.get(PURPOSE_KEY))
                                .and_then(Value::as_str)
                                == Some(MessagePurpose::Working.as_str());
                            rewrite = Some(Rewrite::Merge {
                                id,
                                sent: open.sent,
                                working,
                            });
                        }
                        // The live message is already open on the wire: this START is dropped.
                        continue;
                    }
                    if self.was(&id, Closed::Abandoned) {
                        let to = format!("{id}~final");
                        start.message_id = agui::MessageId::new(to.clone());
                        rewrite = Some(Rewrite::Rename { from: id, to });
                    }
                }
                agui::Event::TextMessageContent(content) => match &rewrite {
                    Some(Rewrite::Merge { id, sent, .. }) if content.message_id.as_str() == id => {
                        let rest = content.delta.strip_prefix(sent.as_str()).map(str::to_owned);
                        let (offset, delta) = match rest {
                            Some(rest) => (utf16_len(sent), rest),
                            None => (0, std::mem::take(&mut content.delta)),
                        };
                        content.delta = delta;
                        content.base.metadata =
                            Some(live_metadata(json!({"offset": offset, "final": true})));
                    }
                    Some(Rewrite::Rename { from, to }) if content.message_id.as_str() == from => {
                        content.message_id = agui::MessageId::new(to.clone());
                    }
                    _ => {}
                },
                agui::Event::TextMessageEnd(end) => match &rewrite {
                    Some(Rewrite::Merge { id, working, .. }) if end.message_id.as_str() == id => {
                        let mut live = json!({"final": true});
                        if *working {
                            live["purpose"] = json!(MessagePurpose::Working.as_str());
                        }
                        end.base.metadata = Some(live_metadata(live));
                        rewrite = None;
                    }
                    Some(Rewrite::Rename { from, to }) if end.message_id.as_str() == from => {
                        end.message_id = agui::MessageId::new(to.clone());
                        rewrite = None;
                    }
                    _ => {}
                },
                agui::Event::SubagentFinished(done) => {
                    if self
                        .open
                        .as_ref()
                        .is_some_and(|o| o.sub == done.subagent_run_id)
                    {
                        self.abandon(&mut out);
                    }
                    if self
                        .reasoning
                        .as_ref()
                        .is_some_and(|o| o.sub == done.subagent_run_id)
                    {
                        self.abandon_reasoning(&mut out);
                    }
                }
                agui::Event::SubagentError(failed) => {
                    if self
                        .open
                        .as_ref()
                        .is_some_and(|o| o.sub == failed.subagent_run_id)
                    {
                        self.abandon(&mut out);
                    }
                    if self
                        .reasoning
                        .as_ref()
                        .is_some_and(|o| o.sub == failed.subagent_run_id)
                    {
                        self.abandon_reasoning(&mut out);
                    }
                }
                agui::Event::RunFinished(_) | agui::Event::RunError(_) => {
                    self.abandon(&mut out);
                    self.abandon_reasoning(&mut out);
                    self.held.clear();
                }
                _ => {}
            }
            out.push(frame);
        }
        if !self.held.is_empty() && projector.run_open() && projector.open_invocation().is_some() {
            for piece in std::mem::take(&mut self.held) {
                self.accept(projector, &piece, &mut out);
            }
        }
        out
    }

    /// Folds one piece of live text in and returns the frames to write now, none of them a resume
    /// point.
    pub fn live(&mut self, projector: &Projector, text: &LiveText) -> Vec<Frame> {
        let mut out = Vec::new();
        self.accept(projector, text, &mut out);
        out
    }

    fn accept(&mut self, projector: &Projector, text: &LiveText, out: &mut Vec<Frame>) {
        let chunk = &text.chunk;
        let id = chunk.message_id.as_str();
        if chunk.kind == LiveKind::Reasoning {
            self.accept_reasoning(projector, text, out);
            return;
        }
        if id.is_empty() || projector.has_message(id) || self.is_closed(id) {
            return;
        }
        // Nothing to say it under while the run is closed (a late piece of a finished reply).
        if !projector.run_open() {
            return;
        }
        let Some((sub, name, actor)) = projector.open_invocation() else {
            self.hold(text.clone());
            return;
        };
        // Words of another agent than the one working here are not this invocation's.
        if name != text.agent.as_str() {
            return;
        }
        if self.open.as_ref().is_none_or(|o| o.id != id) {
            // A message opens at its beginning and with something to show; a piece from the
            // middle waits for the text to be repeated from the start, and one that gives up
            // before anything was said has nothing to end.
            let opens =
                chunk.offset == 0 && !chunk.text.is_empty() && chunk.end != LiveEnd::Abandoned;
            if !opens {
                if chunk.end == LiveEnd::Abandoned {
                    self.remember(id, Closed::Abandoned);
                }
                return;
            }
            // Another stream begins: the one that was open is over.
            self.abandon(out);
            let mut start = TextMessageStartEvent::new(id, TextMessageRole::Assistant);
            start.name = Some(name.to_owned());
            start.subagent_run_id = Some(sub.clone());
            let mut metadata = actor_metadata(actor);
            metadata.insert(LIVE_KEY.to_owned(), json!({}));
            start.base.metadata = Some(metadata);
            out.push(plain(start));
            self.open = Some(OpenLive {
                id: id.to_owned(),
                sub: sub.clone(),
                sent: String::new(),
            });
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };

        // The part of the piece beyond what was said.
        let have = open.sent.len() as u64;
        if chunk.offset <= have {
            let skip = usize::try_from(have - chunk.offset).unwrap_or(usize::MAX);
            let rest = if skip >= chunk.text.len() {
                Some("")
            } else {
                // Not on a character boundary: the pieces do not agree with each other.
                chunk.text.get(skip..)
            };
            if let Some(rest) = rest
                && !rest.is_empty()
                && open.sent.len() + rest.len() <= MAX_LIVE_MESSAGE_BYTES
            {
                let mut content = TextMessageContentEvent::new(open.id.as_str(), rest);
                content.subagent_run_id = Some(open.sub.clone());
                content.base.metadata =
                    Some(live_metadata(json!({"offset": utf16_len(&open.sent)})));
                out.push(plain(content));
                open.sent.push_str(rest);
            }
        }
        // `Last` changes nothing: the message the log says next closes this one.
        if chunk.end == LiveEnd::Abandoned {
            self.abandon(out);
        }
    }

    /// Folds one piece of live reasoning in (ADR 0044): the same rules as a reply's, in AG-UI's
    /// reasoning events. `Last` changes nothing: the log's `agent_reasoning`, which follows at once,
    /// closes the span.
    fn accept_reasoning(&mut self, projector: &Projector, text: &LiveText, out: &mut Vec<Frame>) {
        let chunk = &text.chunk;
        let id = chunk.message_id.as_str();
        if id.is_empty() || projector.has_reasoning(id) || self.is_closed(id) {
            return;
        }
        if !projector.run_open() {
            return;
        }
        let Some((sub, name, actor)) = projector.open_invocation() else {
            self.hold(text.clone());
            return;
        };
        if name != text.agent.as_str() {
            return;
        }
        if self.reasoning.as_ref().is_none_or(|o| o.id != id) {
            let opens =
                chunk.offset == 0 && !chunk.text.is_empty() && chunk.end != LiveEnd::Abandoned;
            if !opens {
                if chunk.end == LiveEnd::Abandoned {
                    self.remember(id, Closed::Abandoned);
                }
                return;
            }
            // Another reasoning begins: the one that was open is over.
            self.abandon_reasoning(out);
            let mut start = ReasoningStartEvent::new(id);
            start.subagent_run_id = Some(sub.clone());
            let mut metadata = actor_metadata(actor);
            metadata.insert(LIVE_KEY.to_owned(), json!({}));
            start.base.metadata = Some(metadata);
            out.push(plain(start));
            let mut message = ReasoningMessageStartEvent::new(id);
            message.subagent_run_id = Some(sub.clone());
            message.base.metadata = Some(live_metadata(json!({})));
            out.push(plain(message));
            self.reasoning = Some(OpenLive {
                id: id.to_owned(),
                sub: sub.clone(),
                sent: String::new(),
            });
        }
        let Some(open) = self.reasoning.as_mut() else {
            return;
        };
        let have = open.sent.len() as u64;
        if chunk.offset <= have {
            let skip = usize::try_from(have - chunk.offset).unwrap_or(usize::MAX);
            let rest = if skip >= chunk.text.len() {
                Some("")
            } else {
                chunk.text.get(skip..)
            };
            if let Some(rest) = rest
                && !rest.is_empty()
                && open.sent.len() + rest.len() <= MAX_LIVE_MESSAGE_BYTES
            {
                let mut content = ReasoningMessageContentEvent::new(open.id.as_str(), rest);
                content.subagent_run_id = Some(open.sub.clone());
                content.base.metadata =
                    Some(live_metadata(json!({"offset": utf16_len(&open.sent)})));
                out.push(plain(content));
                open.sent.push_str(rest);
            }
        }
        if chunk.end == LiveEnd::Abandoned {
            self.abandon_reasoning(out);
        }
    }

    /// Ends the open live reasoning as given up.
    fn abandon_reasoning(&mut self, out: &mut Vec<Frame>) {
        let Some(open) = self.reasoning.take() else {
            return;
        };
        let given_up = json!({"abandoned": true});
        let mut message = ReasoningMessageEndEvent::new(open.id.as_str());
        message.subagent_run_id = Some(open.sub.clone());
        message.base.metadata = Some(live_metadata(given_up.clone()));
        out.push(plain(message));
        let mut end = ReasoningEndEvent::new(open.id.as_str());
        end.subagent_run_id = Some(open.sub);
        end.base.metadata = Some(live_metadata(given_up));
        out.push(plain(end));
        self.remember(&open.id, Closed::Abandoned);
    }

    /// Ends the open live message as given up.
    fn abandon(&mut self, out: &mut Vec<Frame>) {
        let Some(open) = self.open.take() else {
            return;
        };
        let mut end = TextMessageEndEvent::new(open.id.as_str());
        end.subagent_run_id = Some(open.sub);
        end.base.metadata = Some(live_metadata(json!({"abandoned": true})));
        out.push(plain(end));
        self.remember(&open.id, Closed::Abandoned);
    }

    fn hold(&mut self, text: LiveText) {
        if self.held.len() == HELD_MAX {
            self.held.remove(0);
        }
        self.held.push(text);
    }

    fn remember(&mut self, id: &str, how: Closed) {
        if self.closed.len() == CLOSED_MEMORY {
            self.closed.pop_front();
        }
        self.closed.push_back((id.to_owned(), how));
    }

    fn is_closed(&self, id: &str) -> bool {
        self.closed.iter().any(|(closed, _)| closed == id)
    }

    fn was(&self, id: &str, how: Closed) -> bool {
        self.closed
            .iter()
            .any(|(closed, h)| closed == id && *h == how)
    }
}
