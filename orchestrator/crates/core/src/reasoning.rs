//! The reasoning of an agent's model (ADR 0044, `docs/api/text-stream-v1.md` "Reasoning").
//!
//! A model in thinking mode writes what it thinks before it answers, and an agent that lists
//! `text-stream/v1` can send it as chunks marked `kind: "reasoning"`. The orchestrator relays the chunks
//! live (like the words of a reply) and **logs the reasoning once**, whole, as an `agent_reasoning`
//! event, so a reload, an export and a fork's reader see what the screen showed. It is bounded: the
//! log never holds more than [`MAX_REASONING_BYTES`] of it, and says so when it cut it.
//!
//! Reasoning is **not the answer**: it never counts as the turn's words, the title's, the summary
//! the verifier reads or the conversation a fork continues. The core only records it.

use serde::{Deserialize, Serialize};

/// The most bytes of reasoning one `agent_reasoning` event holds. A model that thinks for a minute
/// writes a few thousand words; this keeps a runaway one from filling the log (the live relay carries
/// at most the same, in pieces of [`MAX_LIVE_PIECE_BYTES`](crate::MAX_LIVE_PIECE_BYTES)).
pub const MAX_REASONING_BYTES: usize = 32 * 1024;

/// `data` of an `agent_reasoning`: what the agent's model thought before one of its turns.
///
/// `message_id` is the id the agent gave the reasoning stream (1 to 128 printable bytes), which is
/// also the id of the live reasoning the screen showed, so the live and the logged one are matched
/// as a reply's are. It is never the id of an `agent_message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReasoningData {
    /// The reasoning stream's id.
    pub message_id: String,
    /// The reasoning, at most [`MAX_REASONING_BYTES`] bytes.
    pub text: String,
    /// The text is **not the whole reasoning**: it was cut at the bound, the agent gave up in the
    /// middle of it, or a piece was lost. Absent when it is whole, never `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// `text` as the log may hold it: whole when it fits [`MAX_REASONING_BYTES`], otherwise its head, cut
/// at a character, with `true` for "cut". Control characters other than newline and tab are dropped
/// (Postgres refuses a NUL in JSON text, and nothing else of them is for a person to read).
#[must_use]
pub fn bound_reasoning(text: &str) -> (String, bool) {
    let mut kept = String::with_capacity(text.len().min(MAX_REASONING_BYTES));
    let mut cut = false;
    for c in text.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        if kept.len() + c.len_utf8() > MAX_REASONING_BYTES {
            cut = true;
            break;
        }
        kept.push(c);
    }
    (kept, cut)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_short_reasoning_is_kept_whole() {
        assert_eq!(
            bound_reasoning("The user wants Fibonacci.\nIn Rust."),
            ("The user wants Fibonacci.\nIn Rust.".to_owned(), false)
        );
    }

    #[test]
    fn control_characters_are_dropped_but_not_line_breaks() {
        assert_eq!(
            bound_reasoning("a\u{0}b\u{7}c\td\ne"),
            ("abc\td\ne".to_owned(), false)
        );
    }

    #[test]
    fn a_long_reasoning_is_cut_at_a_character_and_says_so() {
        let long = "é".repeat(MAX_REASONING_BYTES);
        let (kept, cut) = bound_reasoning(&long);
        assert!(cut);
        assert_eq!(kept.len(), MAX_REASONING_BYTES);
        assert!(kept.chars().all(|c| c == 'é'));
        // Exactly the bound is whole.
        let (kept, cut) = bound_reasoning(&"a".repeat(MAX_REASONING_BYTES));
        assert!(!cut);
        assert_eq!(kept.len(), MAX_REASONING_BYTES);
        // A four-byte character that would straddle the bound is left out, never split.
        let straddling = format!("{}🙂", "a".repeat(MAX_REASONING_BYTES - 2));
        let (kept, cut) = bound_reasoning(&straddling);
        assert!(cut);
        assert_eq!(kept.len(), MAX_REASONING_BYTES - 2);
    }

    #[test]
    fn the_wire_shape_says_truncated_only_when_it_is() {
        let whole = AgentReasoningData {
            message_id: "r1".into(),
            text: "t".into(),
            truncated: false,
        };
        assert_eq!(
            serde_json::to_value(&whole).expect("serialises"),
            serde_json::json!({"messageId": "r1", "text": "t"})
        );
        let cut = AgentReasoningData {
            truncated: true,
            ..whole
        };
        assert_eq!(
            serde_json::to_value(&cut).expect("serialises"),
            serde_json::json!({"messageId": "r1", "text": "t", "truncated": true})
        );
        let bare: AgentReasoningData =
            serde_json::from_value(serde_json::json!({"messageId": "r1", "text": "t"}))
                .expect("reads");
        assert!(!bare.truncated);
    }
}
