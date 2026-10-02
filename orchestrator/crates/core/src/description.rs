//! A thread's description: a sentence or two on what the conversation is about now, which a title
//! of six words cannot say (ADR 0035).
//!
//! The model is asked for one when a job ends or pauses for the person (the thread is `done` or
//! `blocked`), at most once per job, and only when the conversation has grown by
//! [`DEFAULT_MIN_NEW_MESSAGES`] messages (or the deployment's number) since the last one. The
//! thread remembers whose description it has in its job ledger ([`DescriptionLedger`]): a person's
//! edit, an empty one included, is final, and a description the model writes after it is dropped.
//! What the model says ([`clean_description`]) is data: one paragraph of plain text, never an
//! instruction.

use serde::{Deserialize, Serialize};

use crate::event::{Event, EventBody};
use crate::title::{agent_words, trim_markdown_edges};

/// Most characters of a description a person can write (and that the log keeps).
pub const MAX_DESCRIPTION_CHARS: usize = 500;

/// The default of `tasks.description.maxChars`: where a model's description is cut, at a word.
pub const DEFAULT_DESCRIPTION_CHARS: usize = 300;

/// The default of `tasks.description.recompute.minNewMessages`: how many messages the
/// conversation has to have grown by since the last description before a new one is asked for.
pub const DEFAULT_MIN_NEW_MESSAGES: u32 = 4;

/// Whose words the thread's description is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptionSource {
    /// Nobody's: what every thread starts with.
    #[default]
    None,
    /// A description the orchestrator had written for the conversation.
    Model,
    /// A description a person wrote, or cleared. Nothing else replaces it.
    User,
}

impl DescriptionSource {
    /// Whether this is what every thread starts with, which the ledger leaves out.
    pub fn is_default(&self) -> bool {
        *self == DescriptionSource::default()
    }
}

/// Who wrote a description that a `thread_described` event records (contract
/// `ThreadDescribedData.source`). Closed (ADR 0004): there is no event for no description, which
/// is what a thread starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescribedBy {
    /// The orchestrator, from the conversation.
    Model,
    /// A person.
    User,
}

impl DescribedBy {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            DescribedBy::Model => "model",
            DescribedBy::User => "user",
        }
    }
}

impl From<DescribedBy> for DescriptionSource {
    fn from(by: DescribedBy) -> Self {
        match by {
            DescribedBy::Model => DescriptionSource::Model,
            DescribedBy::User => DescriptionSource::User,
        }
    }
}

/// `data` of a `thread_described`: the thread has a new description. An empty one is the person
/// clearing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadDescribedData {
    /// The new description; empty when a person cleared it.
    pub description: String,
    /// Who wrote it.
    pub source: DescribedBy,
}

/// The thread's memory of its description (`Job.description`). Belongs to the conversation, not to
/// a job: [`Job::next`](crate::Job::next) keeps it. A ledger stored before the field existed has
/// no description, which is what its thread has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DescriptionLedger {
    #[serde(skip_serializing_if = "DescriptionSource::is_default")]
    source: DescriptionSource,
    /// The job that last asked the model for a description: a job asks at most once.
    #[serde(skip_serializing_if = "is_zero")]
    asked_job: u32,
    /// The ask of `asked_job` is in flight: its answer has not come yet.
    #[serde(skip_serializing_if = "is_false")]
    pending: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl DescriptionLedger {
    /// A ledger of a thread with no description, which the log leaves out.
    pub fn is_empty(&self) -> bool {
        self.source.is_default() && self.asked_job == 0 && !self.pending
    }

    /// The ledger of a thread whose description is `source`'s, with nothing asked.
    pub fn of(source: DescriptionSource) -> Self {
        DescriptionLedger {
            source,
            asked_job: 0,
            pending: false,
        }
    }

    /// The ledger a fork starts with (ADR 0029): the parent's source, with nothing in flight and
    /// no job asked. The fork's jobs are numbered from the newest job the copy holds, which may be
    /// one the parent asked in, so the parent's job number says nothing about the fork's.
    pub(crate) fn inherited(self) -> Self {
        DescriptionLedger::of(self.source)
    }

    /// Whose description the thread has.
    pub fn source(&self) -> DescriptionSource {
        self.source
    }

    /// Whether the model may be asked at the end of `job`: the thread's description is not a
    /// person's (final, even when empty) and this job has not asked.
    pub fn may_ask(&self, job: u32) -> bool {
        self.source != DescriptionSource::User && self.asked_job != job
    }

    /// The model is asked at the end of `job`.
    pub(crate) fn asked(&mut self, job: u32) {
        self.asked_job = job;
        self.pending = true;
    }

    /// The ask of `job` is answered, with a description or without. `false` for an answer to an
    /// ask that is not the one in flight (a stale or repeated one), which the core ignores.
    pub(crate) fn answered(&mut self, job: u32) -> bool {
        let current = self.pending && self.asked_job == job;
        if current {
            self.pending = false;
        }
        current
    }

    /// The thread has a description that `by` wrote.
    pub(crate) fn written_by(&mut self, by: DescribedBy) {
        self.source = by.into();
    }
}

/// Why a description a person wrote cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DescriptionError {
    /// More than [`MAX_DESCRIPTION_CHARS`] characters.
    #[error("the description is longer than {MAX_DESCRIPTION_CHARS} characters")]
    TooLong,
    /// A control character: a description is one line of text.
    #[error("the description has a control character")]
    Control,
}

/// The description a person's edit asks for: the text with the space around it cut, when it is
/// one line of 0 to [`MAX_DESCRIPTION_CHARS`] characters with no control character. Empty is the
/// person clearing it.
///
/// # Errors
/// [`DescriptionError`] for a too long or a multi-line description.
pub fn check_description(raw: &str) -> Result<String, DescriptionError> {
    let description = raw.trim();
    if description.chars().any(char::is_control) {
        return Err(DescriptionError::Control);
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(DescriptionError::TooLong);
    }
    Ok(description.to_owned())
}

/// The description a model's answer stands for, or `None` when it has none: the first paragraph
/// of the answer, in one line, with the quotes, Markdown and control characters a model likes to
/// add taken off the edges, spaces collapsed, and at most `max_chars` characters, cut at a word.
/// An empty answer and `NONE` (the model's way of saying there is nothing to describe yet) are
/// `None`.
pub fn clean_description(raw: &str, max_chars: usize) -> Option<String> {
    // The first paragraph: everything up to the first blank line.
    let mut paragraph = Vec::new();
    for line in raw.lines().map(str::trim) {
        if line.is_empty() {
            if paragraph.is_empty() {
                continue;
            }
            break;
        }
        paragraph.push(line);
    }
    let kept: String = paragraph
        .join(" ")
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let description = trim_markdown_edges(&kept)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if description.is_empty()
        || description
            .trim_end_matches('.')
            .eq_ignore_ascii_case("none")
    {
        return None;
    }
    if description.chars().count() <= max_chars {
        return Some(description);
    }
    // too long: cut at the last space that leaves room for the ellipsis, else in the middle of a word
    let head: String = description
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    let cut = match head.rfind(' ') {
        Some(i) if i > 0 => &head[..i],
        _ => head.as_str(),
    };
    let cut = cut.trim_end_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation());
    (!cut.is_empty()).then(|| format!("{cut}…"))
}

/// One message of the conversation, as a description request counts and shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Message<'a> {
    /// A person wrote it (else an agent said it).
    pub person: bool,
    /// What was said, trimmed and not empty.
    pub text: &'a str,
}

/// The messages of the conversation in `events`, in order: what the people wrote and what the
/// agent said as final words (a final message, or what a status that ends or interrupts the turn
/// says). The same words said twice in a row by the agent (a final message, then the status that
/// ends the turn with them) are one.
pub(crate) fn conversation_messages(events: &[Event]) -> Vec<Message<'_>> {
    let mut out = Vec::new();
    let mut last_agent: Option<&str> = None;
    for event in events {
        match &event.body {
            EventBody::UserMessage(m) => {
                let text = m.text.trim();
                if !text.is_empty() {
                    out.push(Message { person: true, text });
                    last_agent = None;
                }
            }
            other => {
                if let Some(words) = agent_words(other) {
                    let text = words.trim();
                    if last_agent != Some(text) {
                        out.push(Message {
                            person: false,
                            text,
                        });
                        last_agent = Some(text);
                    }
                }
            }
        }
    }
    out
}

/// How many messages the conversation in `events` has grown by since the last
/// `thread_described` of the log (all of them when it has none): the number a description is
/// asked for against `minNewMessages`. Pure: it reads only the log it is given, which is the
/// part of the thread the caller read.
pub fn messages_since_description(events: &[Event]) -> usize {
    let since = events
        .iter()
        .rposition(|e| matches!(e.body, EventBody::ThreadDescribed(_)))
        .map_or(0, |i| i + 1);
    conversation_messages(&events[since..]).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_a_person_wrote_is_trimmed_and_may_be_empty() {
        assert_eq!(
            check_description("  Moving the build to Rust \t").as_deref(),
            Ok("Moving the build to Rust")
        );
        assert_eq!(check_description("").as_deref(), Ok(""));
        assert_eq!(check_description(" \u{a0} ").as_deref(), Ok(""));
    }

    #[test]
    fn a_description_is_one_line_of_at_most_the_limit() {
        assert_eq!(
            check_description("two\nlines"),
            Err(DescriptionError::Control)
        );
        assert_eq!(
            check_description("bell\u{7}"),
            Err(DescriptionError::Control)
        );
        let at_limit = "é".repeat(MAX_DESCRIPTION_CHARS);
        assert_eq!(
            check_description(&at_limit).as_deref(),
            Ok(at_limit.as_str())
        );
        assert_eq!(
            check_description(&"é".repeat(MAX_DESCRIPTION_CHARS + 1)),
            Err(DescriptionError::TooLong)
        );
    }

    #[test]
    fn what_a_model_answers_is_one_plain_paragraph() {
        let clean = |raw: &str| clean_description(raw, 300);
        assert_eq!(
            clean("  \"The person is moving the build to Rust.\"  ").as_deref(),
            Some("The person is moving the build to Rust.")
        );
        assert_eq!(
            clean("**Moving the build.**\nThe tests pass.\n\nA second paragraph.").as_deref(),
            Some("Moving the build.** The tests pass.")
        );
        assert_eq!(clean("# Heading\n").as_deref(), Some("Heading"));
        assert_eq!(clean("a \u{7}b\t c").as_deref(), Some("a b c"));
        for none in ["", "  \n ", "NONE", "none.", "\"None\""] {
            assert_eq!(clean(none), None, "{none:?}");
        }
    }

    #[test]
    fn a_long_description_is_cut_at_a_word_with_an_ellipsis() {
        let long = "word ".repeat(100);
        let cut = clean_description(&long, 40).unwrap_or_default();
        assert!(cut.chars().count() <= 40, "{cut}");
        assert!(cut.ends_with("word…"), "{cut}");
        let one = "x".repeat(100);
        let cut = clean_description(&one, 20).unwrap_or_default();
        assert_eq!(cut.chars().count(), 20, "{cut}");
    }

    #[test]
    fn the_ledger_of_a_thread_with_no_description_is_empty_and_says_nothing() {
        let ledger = DescriptionLedger::default();
        assert!(ledger.is_empty());
        assert_eq!(
            serde_json::to_value(ledger).ok(),
            Some(serde_json::json!({}))
        );
        let mut asked = DescriptionLedger::default();
        asked.asked(3);
        asked.written_by(DescribedBy::Model);
        assert_eq!(
            serde_json::to_value(asked).ok(),
            Some(serde_json::json!({"source": "model", "askedJob": 3, "pending": true}))
        );
    }

    #[test]
    fn a_job_asks_once_and_its_answer_counts_once() {
        let mut ledger = DescriptionLedger::default();
        assert!(ledger.may_ask(1));
        ledger.asked(1);
        assert!(!ledger.may_ask(1), "once per job");
        assert!(ledger.may_ask(2));
        assert!(
            !ledger.answered(2),
            "an answer to another job's ask is stale"
        );
        assert!(ledger.answered(1));
        assert!(!ledger.answered(1), "a repeated answer is stale");
    }

    #[test]
    fn a_persons_description_is_final_and_a_fork_keeps_only_whose_it_is() {
        let mut ledger = DescriptionLedger::default();
        ledger.asked(2);
        ledger.written_by(DescribedBy::User);
        assert!(!ledger.may_ask(3));
        let fork = ledger.inherited();
        assert_eq!(fork.source(), DescriptionSource::User);
        assert_eq!(fork, DescriptionLedger::of(DescriptionSource::User));
        let mut model = DescriptionLedger::default();
        model.asked(2);
        model.written_by(DescribedBy::Model);
        let fork = model.inherited();
        assert!(fork.may_ask(2), "the fork's job 2 is not the parent's");
    }
}
