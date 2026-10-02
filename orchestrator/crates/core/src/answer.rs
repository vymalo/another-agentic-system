//! The answer a turn announced (`turn_output`, ADR 0031, amended 2026-10-02).
//!
//! An agent that lists `thread-tools/v1` can say "this is my answer" before it is done, with the
//! `turn_output` tool: the orchestrator records it as an `agent_message` marked
//! `purpose: answer, via: turn_output` ([`announce`]). From then on **nothing else the agent says
//! in the turn is the answer**: the words of a stated stream on a status that ends the turn, a
//! plain `Message`, and the words of the status itself are marked `working` (kept, reachable, just
//! not the answer), unless they repeat what was said last, in which case they are not said twice.
//!
//! What the core needs to know lives in the job ledger ([`AnswerLedger`], `Job.answer`), so any
//! replica decides the same way (ADR 0001): whether the turn has announced an answer, which token
//! announced it and how many times (for the message id), and a digest of the last words said, to
//! tell a repeat. A new delegation (the person writes, acts on a card, a rework) starts a new turn
//! and forgets it ([`AnswerLedger::reset`]), and so does a new job.
//!
//! **Replacement.** A later `turn_output` in the same turn is another `agent_message` with
//! `via: turn_output`. The log is append-only (ADR 0001), so the earlier one is not rewritten:
//! the rule is that **the answer of a turn is the last message marked `answer` in it**, and an
//! earlier one is working text, whatever its own mark says. A reader applies the rule where it
//! draws the text (`docs/api/agui.md`, "The agent's words").

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::event::{Actor, AgentMessageData, AnswerVia, EventBody, MessagePurpose};
use crate::gate::Job;
use crate::thread::ThreadState;
use crate::transition::{Command, TransitionError, append};

/// Most bytes of an announced answer: what the `turn_output` tool accepts.
pub const MAX_ANSWER_BYTES: usize = 65_536;

/// Why a text cannot be an announced answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AnswerError {
    /// Nothing but white space.
    #[error("text must not be empty")]
    Empty,
    /// More than [`MAX_ANSWER_BYTES`].
    #[error("text must be at most {MAX_ANSWER_BYTES} bytes")]
    TooLong,
}

/// Checks the text of an announced answer: not empty (white space is nothing), at most
/// [`MAX_ANSWER_BYTES`] bytes.
///
/// # Errors
///
/// [`AnswerError`] for an empty or oversize text.
pub fn check_answer(text: &str) -> Result<(), AnswerError> {
    if text.trim().is_empty() {
        return Err(AnswerError::Empty);
    }
    if text.len() > MAX_ANSWER_BYTES {
        return Err(AnswerError::TooLong);
    }
    Ok(())
}

/// What the job remembers of the answer its current turn announced. Empty (and left out of the
/// stored ledger) for a turn that announced none, which is every turn of an agent that does not
/// call `turn_output`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnswerLedger {
    /// The `jti` of the token that announced (the A2A message id the call was made under).
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    /// How many answers that token announced in this turn: the `<n>` of `out-<jti>-<n>`.
    #[serde(skip_serializing_if = "is_zero")]
    calls: u32,
    /// A digest of the last words said since the first announcement (the announcement itself, a
    /// working message, words the core wrote for a status), to tell a repeat of them.
    #[serde(skip_serializing_if = "Option::is_none")]
    said: Option<String>,
    /// How many messages the core wrote itself for the words of a status (the `<n>` of
    /// `out-<jti>-words-<n>`).
    #[serde(skip_serializing_if = "is_zero")]
    words: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The hex of the SHA-256 of the text without its surrounding white space.
fn digest(text: &str) -> String {
    let hash = Sha256::digest(text.trim().as_bytes());
    let mut out = String::with_capacity(2 * hash.len());
    for byte in hash {
        // Writing to a String does not fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

impl AnswerLedger {
    /// A ledger of a turn that announced nothing.
    pub fn is_empty(&self) -> bool {
        self.calls == 0 && self.words == 0 && self.token.is_none() && self.said.is_none()
    }

    /// Whether the turn has announced its answer: from then on nothing else it says is the answer.
    pub fn is_announced(&self) -> bool {
        self.calls > 0
    }

    /// A new turn: the person wrote, acted on a card, or the gate sent the agent back.
    pub(crate) fn reset(&mut self) {
        *self = AnswerLedger::default();
    }

    /// What the words of an agent message become in this turn: `None` when they are to be dropped
    /// (the same words were just said), else the purpose to write.
    ///
    /// Before an announcement nothing changes: the purpose is the adapter's. After one, no text
    /// is the answer (working it stays, an answer or an unmarked text becomes working).
    pub(crate) fn message(
        &mut self,
        purpose: Option<MessagePurpose>,
        is_final: bool,
        text: &str,
    ) -> Option<Option<MessagePurpose>> {
        if !self.is_announced() {
            return Some(purpose);
        }
        if is_final && !self.say(text) {
            return None;
        }
        Some(Some(MessagePurpose::Working))
    }

    /// The `agent_message` the core writes for the words of a status that ends the turn, when the
    /// turn has announced its answer: working text, so that no consumer says them as an answer
    /// (they are the status's `detail`, which stays, and a consumer says status words that no
    /// final message said). `None` for words that were just said, and when there is nothing to
    /// say.
    pub(crate) fn status_words(&mut self, actor: &Actor, detail: Option<&str>) -> Option<Command> {
        let words = detail.filter(|d| !d.trim().is_empty())?;
        if !self.is_announced() || !self.say(words) {
            return None;
        }
        self.words += 1;
        let token = self.token.as_deref().unwrap_or_default();
        Some(append(
            actor.clone(),
            EventBody::AgentMessage(AgentMessageData {
                text: words.to_owned(),
                message_id: format!("out-{token}-words-{}", self.words),
                is_final: true,
                purpose: Some(MessagePurpose::Working),
                via: None,
            }),
        ))
    }

    /// Records `text` as the last words said; `false` when it is what was said last already.
    fn say(&mut self, text: &str) -> bool {
        let d = digest(text);
        if self.said.as_deref() == Some(d.as_str()) {
            return false;
        }
        self.said = Some(d);
        true
    }
}

/// An agent announces its answer (`Input::Answer`, from the `turn_output` tool): the
/// `agent_message` that says so.
///
/// Accepted only while the thread is `queued` or `working`, for the current job, and under the
/// token that announced before in this turn (another token is a turn that is over: a later
/// message has its own). The text is checked again here ([`check_answer`]) and the caller is told
/// by the application's validation first.
///
/// # Errors
///
/// [`TransitionError::InvalidInState`] when the turn is over (any other state, an earlier job, a
/// token of an earlier turn).
pub(crate) fn announce(
    state: ThreadState,
    job: &mut Job,
    actor: &Actor,
    text: &str,
    claimed_job: u32,
    token: &str,
) -> Result<Vec<Command>, TransitionError> {
    let over = TransitionError::InvalidInState {
        state,
        input: "answer",
    };
    match state {
        ThreadState::Queued | ThreadState::Working => {}
        ThreadState::Blocked
        | ThreadState::Verifying
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => return Err(over),
    }
    if claimed_job != job.number
        || job
            .answer
            .token
            .as_deref()
            .is_some_and(|announced_by| announced_by != token)
        || check_answer(text).is_err()
    {
        return Err(over);
    }
    let ledger = &mut job.answer;
    ledger.token = Some(token.to_owned());
    ledger.calls = ledger.calls.saturating_add(1);
    ledger.said = Some(digest(text));
    let message_id = format!("out-{token}-{}", ledger.calls);
    crate::transition::note_summary(job, text);
    Ok(vec![append(
        actor.clone(),
        EventBody::AgentMessage(AgentMessageData {
            text: text.to_owned(),
            message_id,
            is_final: true,
            purpose: Some(MessagePurpose::Answer),
            via: Some(AnswerVia::TurnOutput),
        }),
    )])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_one_to_sixty_five_thousand_bytes_of_something() {
        assert_eq!(check_answer(""), Err(AnswerError::Empty));
        assert_eq!(check_answer(" \n\t "), Err(AnswerError::Empty));
        assert_eq!(check_answer("x"), Ok(()));
        assert_eq!(check_answer(&"x".repeat(MAX_ANSWER_BYTES)), Ok(()));
        assert_eq!(
            check_answer(&"x".repeat(MAX_ANSWER_BYTES + 1)),
            Err(AnswerError::TooLong)
        );
        // bytes, not characters
        assert_eq!(
            check_answer(&"é".repeat(MAX_ANSWER_BYTES / 2 + 1)),
            Err(AnswerError::TooLong)
        );
    }

    #[test]
    fn a_digest_ignores_the_white_space_around_the_words() {
        assert_eq!(digest("Done.\n"), digest("  Done."));
        assert_ne!(digest("Done."), digest("Done"));
    }

    #[test]
    fn an_empty_ledger_is_left_out_and_a_used_one_is_not() {
        let mut ledger = AnswerLedger::default();
        assert!(ledger.is_empty());
        assert!(!ledger.is_announced());
        assert_eq!(serde_json::to_string(&ledger).unwrap(), "{}");
        ledger.calls = 1;
        assert!(!ledger.is_empty());
        assert!(ledger.is_announced());
        ledger.reset();
        assert!(ledger.is_empty());
    }
}
