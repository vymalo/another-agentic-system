//! A mention: an agent a person named in a message, as a structured reference (ADR 0026,
//! `docs/api/mentions-v1.md`).
//!
//! The reference says **which agent** was meant (`agent_id`, the identity of the mention: the
//! label is never read as one) and **where** its label stands in the message text. Offsets count
//! **UTF-16 code units**, what a JavaScript string indexes, because the composer is the only
//! producer and builds the message in one. The checks of a reference that arrives (the shape, the
//! label against the text, the registry, the person's roles) are the application's
//! (`orch_app::mentions`); the core holds what it **keeps**: the reference as it was sent, and
//! the arithmetic that moves it when messages are joined.

use serde::{Deserialize, Serialize};

use crate::ids::AgentId;

/// The most references one message may carry.
pub const MAX_MENTIONS: usize = 16;

/// The most UTF-16 code units a label may have, the `@` included.
pub const MAX_MENTION_LABEL_UNITS: usize = 64;

/// How many UTF-16 code units `text` has: the unit of a mention's offsets.
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// One mentioned agent, as the composer sent it and the `user_message` event stores it
/// (`data.mentions`, omitted when a message has none).
///
/// `text[start..end]`, counted in UTF-16 code units, is `label`; `start` is inclusive and `end`
/// exclusive. The core stores a reference **as sent**: it never rewrites a label or an offset of
/// a message, and whether the offsets fit the text was checked before the event was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    /// The id the registry gives the agent (ADR 0022).
    pub agent_id: AgentId,
    /// The text shown for it, as it stands in the message (`@researcher`).
    pub label: String,
    /// Where the label begins in the message, in UTF-16 code units.
    pub start: u32,
    /// Where the label ends (exclusive), in UTF-16 code units.
    pub end: u32,
    /// The agent's card URL as the composer saw it in the agent list. When given it equalled the
    /// registry's at the time the message was checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_url: Option<String>,
}

impl Mention {
    /// The same reference `by` code units further on: where it stands in a text that has `by`
    /// code units in front of the one it was written in (a message joined behind another).
    #[must_use]
    pub fn shifted(&self, by: usize) -> Mention {
        let by = u32::try_from(by).unwrap_or(u32::MAX);
        Mention {
            start: self.start.saturating_add(by),
            end: self.end.saturating_add(by),
            ..self.clone()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_character_outside_the_basic_plane_is_two_units() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("\u{1F604}"), 2);
        assert_eq!(
            utf16_len("e\u{301}"),
            2,
            "a combining mark is a unit of its own"
        );
        assert_eq!(utf16_len(""), 0);
    }

    #[test]
    fn a_reference_is_stored_as_sent_and_omits_what_is_absent() {
        let mention = Mention {
            agent_id: AgentId::new("mock-researcher"),
            label: "@researcher".to_owned(),
            start: 3,
            end: 14,
            card_url: None,
        };
        let json = serde_json::to_value(&mention).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"agentId": "mock-researcher", "label": "@researcher", "start": 3, "end": 14})
        );
        let back: Mention = serde_json::from_value(json).unwrap();
        assert_eq!(back, mention);
        let with_card = Mention {
            card_url: Some("http://r/card".to_owned()),
            ..mention
        };
        let json = serde_json::to_value(&with_card).unwrap();
        assert_eq!(json["cardUrl"], "http://r/card");
    }

    #[test]
    fn shifting_moves_both_ends_and_keeps_the_rest() {
        let mention = Mention {
            agent_id: AgentId::new("a"),
            label: "@a".to_owned(),
            start: 4,
            end: 6,
            card_url: Some("u".to_owned()),
        };
        let moved = mention.shifted(10);
        assert_eq!((moved.start, moved.end), (14, 16));
        assert_eq!(moved.agent_id, mention.agent_id);
        assert_eq!(moved.card_url, mention.card_url);
    }
}
