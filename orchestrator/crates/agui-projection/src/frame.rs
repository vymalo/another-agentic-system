use std::collections::BTreeSet;

use orch_agui_proto as agui;

/// One AG-UI event, ready to be written to a stream, and where a client may resume from.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The AG-UI event.
    pub event: agui::Event,
    /// `Some(seq)` when this is the last frame produced for log event `seq` **and** no text
    /// message is open after it: the SSE `id:` to write. Reconnecting with that id yields
    /// exactly the frames after this one, and never splits a message.
    pub resume_id: Option<i64>,
}

/// Who a projection is for.
///
/// The two audiences see the same story. They differ in one thing: the **requester** (the POST
/// that sent the input) already holds the user messages it put in its own `RunAgentInput`, and
/// re-streaming one would *append* to it in the client's transcript, so their text is skipped. A
/// **viewer** (a connect stream) holds nothing and gets everything.
#[derive(Debug, Clone, Copy)]
pub enum Audience<'a> {
    /// A connect stream: everything is new material.
    Viewer,
    /// The run POST that sent the input.
    Requester {
        /// Ids of the messages in the request's `messages` (see
        /// [`held_message_ids`](crate::held_message_ids)).
        held_message_ids: &'a BTreeSet<String>,
    },
}

impl Audience<'_> {
    /// Whether a user message with this id is already in the audience's transcript.
    pub(crate) fn holds(&self, message_id: &str) -> bool {
        match self {
            Audience::Viewer => false,
            Audience::Requester { held_message_ids } => held_message_ids.contains(message_id),
        }
    }
}
