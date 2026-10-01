//! Live text: the words of an agent's reply while it is being written (ADR 0027).
//!
//! The log holds an agent's **final** words only (`agent_message`). A [`LiveText`] is what travels
//! *before* that: a piece of the reply, relayed between processes and shown by whoever is looking
//! at the thread, never stored and never a resume point. Nothing here decides anything: these are
//! plain data, so the port that carries them (`Wakeup`, in `orch-ports`) and
//! the code that folds them into frames can agree on one shape without the core knowing about
//! either.

use crate::ids::{AgentId, ThreadId};

/// Largest piece of text one [`LiveChunk`] carries when the orchestrator relays it, in bytes. A
/// relay cuts its pieces to this size; an implementation of the port may split one further to fit
/// its transport.
pub const MAX_LIVE_PIECE_BYTES: usize = 6 * 1024;

/// Whether a piece is the end of the stream it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveEnd {
    /// More may follow.
    Open,
    /// The agent said it is the last piece: its whole text now follows in the log.
    Last,
    /// The agent gave up on the reply (the generation failed): no final text follows from this
    /// stream, and whoever shows it should say so rather than leave it hanging.
    Abandoned,
}

impl LiveEnd {
    /// The one-letter code an implementation of the port may use on the wire: `o`, `l`, `a`.
    pub const fn code(self) -> char {
        match self {
            LiveEnd::Open => 'o',
            LiveEnd::Last => 'l',
            LiveEnd::Abandoned => 'a',
        }
    }

    /// The end a [`code`](Self::code) names.
    pub const fn from_code(code: char) -> Option<Self> {
        match code {
            'o' => Some(LiveEnd::Open),
            'l' => Some(LiveEnd::Last),
            'a' => Some(LiveEnd::Abandoned),
            _ => None,
        }
    }
}

/// One piece of a streamed reply.
///
/// `message_id` is the id the agent gave the stream; the log's `agent_message` that states the
/// whole text has the same id, which is how the live text and the persisted one are matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveChunk {
    /// The stream's id, which is also the id of the message that will state its whole text.
    pub message_id: String,
    /// Where `text` begins in the whole text, counted in **UTF-8 bytes**. A piece whose offset is
    /// not the end of what a receiver already has is either an overlap (a repeat, trimmed) or a gap
    /// (something was lost, until a later piece starts from the beginning again).
    pub offset: u64,
    /// The piece itself. May be empty (the last piece of a stream often is).
    pub text: String,
    /// Whether the stream ends with this piece.
    pub end: LiveEnd,
}

/// A piece of live text and what it belongs to: the unit the `Wakeup` port
/// (`orch-ports`) publishes and delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveText {
    /// The thread whose reply this is.
    pub thread: ThreadId,
    /// The agent writing it.
    pub agent: AgentId,
    /// The piece.
    pub chunk: LiveChunk,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_end_round_trips_through_its_code() {
        for end in [LiveEnd::Open, LiveEnd::Last, LiveEnd::Abandoned] {
            assert_eq!(LiveEnd::from_code(end.code()), Some(end));
        }
        assert_eq!(LiveEnd::from_code('x'), None);
        assert_eq!(LiveEnd::from_code('O'), None);
    }
}
