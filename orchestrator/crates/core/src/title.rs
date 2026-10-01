//! A thread's title: who wrote it, and what a person's rename must look like.
//!
//! A thread is created with the first words of its first message as its title. A person can
//! rename it at any time, in any state of the thread (`Input::Rename`); the log keeps who did, as a
//! `thread_titled` event. The thread remembers whose title it has in its job ledger
//! ([`TitleLedger`]), so that a title a person wrote is never replaced by another writer.

use serde::{Deserialize, Serialize};

/// Most characters a title can have.
pub const MAX_TITLE_CHARS: usize = 200;

/// Whose words the thread's title is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    /// The first words of the first message: what every thread starts with.
    #[default]
    FirstMessage,
    /// A title the orchestrator had written for the conversation.
    Model,
    /// A title a person wrote. Nothing else replaces it.
    User,
}

impl TitleSource {
    /// Whether this is what every thread starts with, which the ledger leaves out.
    pub fn is_default(&self) -> bool {
        *self == TitleSource::default()
    }
}

/// Who wrote a title that a `thread_titled` event records (contract `ThreadTitledData.source`).
/// Closed (ADR 0004): there is no event for the first message's words, which are the title a thread
/// is created with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitledBy {
    /// The orchestrator, from the conversation.
    Model,
    /// A person.
    User,
}

impl TitledBy {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            TitledBy::Model => "model",
            TitledBy::User => "user",
        }
    }
}

impl From<TitledBy> for TitleSource {
    fn from(by: TitledBy) -> Self {
        match by {
            TitledBy::Model => TitleSource::Model,
            TitledBy::User => TitleSource::User,
        }
    }
}

/// `data` of a `thread_titled`: the thread has a new title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTitledData {
    /// The new title.
    pub title: String,
    /// Who wrote it.
    pub source: TitledBy,
}

/// The thread's memory of its title (`Job.title`). Belongs to the conversation, not to a job:
/// [`Job::next`](crate::Job::next) keeps it. A ledger stored before the field existed has the
/// first message's words, which is what its thread has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TitleLedger {
    #[serde(skip_serializing_if = "TitleSource::is_default")]
    source: TitleSource,
}

impl TitleLedger {
    /// A ledger of a thread that kept its first words, which is what the log leaves out.
    pub fn is_empty(&self) -> bool {
        self.source.is_default()
    }

    /// The ledger of a thread whose title is `source`'s.
    pub fn of(source: TitleSource) -> Self {
        TitleLedger { source }
    }

    /// Whose title the thread has.
    pub fn source(&self) -> TitleSource {
        self.source
    }

    /// The thread has a title that `by` wrote.
    pub(crate) fn written_by(&mut self, by: TitledBy) {
        self.source = by.into();
    }
}

/// Why a title cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TitleError {
    /// Nothing is left of it once the spaces around it are cut.
    #[error("the title is empty")]
    Empty,
    /// More than [`MAX_TITLE_CHARS`] characters.
    #[error("the title is longer than {MAX_TITLE_CHARS} characters")]
    TooLong,
    /// A control character: a title is one line of text.
    #[error("the title has a control character")]
    Control,
}

/// The title a person's rename asks for: the text with the space around it cut, when it is one
/// line of 1 to [`MAX_TITLE_CHARS`] characters with no control character.
///
/// # Errors
/// [`TitleError`] for an empty, a too long or a multi-line title.
pub fn check_title(raw: &str) -> Result<String, TitleError> {
    let title = raw.trim();
    if title.is_empty() {
        return Err(TitleError::Empty);
    }
    if title.chars().any(char::is_control) {
        return Err(TitleError::Control);
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(TitleError::TooLong);
    }
    Ok(title.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_trimmed() {
        assert_eq!(
            check_title("  Fix the build \t").as_deref(),
            Ok("Fix the build")
        );
    }

    #[test]
    fn nothing_but_space_is_empty() {
        assert_eq!(check_title(""), Err(TitleError::Empty));
        assert_eq!(check_title(" \u{a0} \n "), Err(TitleError::Empty));
    }

    #[test]
    fn a_control_character_inside_is_refused() {
        assert_eq!(check_title("two\nlines"), Err(TitleError::Control));
        assert_eq!(check_title("bell\u{7}"), Err(TitleError::Control));
        assert_eq!(check_title("nul\u{0}"), Err(TitleError::Control));
        assert_eq!(check_title("tab\tinside"), Err(TitleError::Control));
    }

    #[test]
    fn the_limit_counts_characters_not_bytes() {
        let at_limit = "é".repeat(MAX_TITLE_CHARS);
        assert_eq!(check_title(&at_limit).as_deref(), Ok(at_limit.as_str()));
        assert_eq!(
            check_title(&"é".repeat(MAX_TITLE_CHARS + 1)),
            Err(TitleError::TooLong)
        );
    }

    #[test]
    fn the_ledger_of_a_thread_that_kept_its_first_words_is_empty_and_says_nothing() {
        let ledger = TitleLedger::default();
        assert!(ledger.is_empty());
        assert_eq!(
            serde_json::to_value(ledger).ok(),
            Some(serde_json::json!({}))
        );
        let mut written = TitleLedger::default();
        written.written_by(TitledBy::User);
        assert_eq!(
            serde_json::to_value(written).ok(),
            Some(serde_json::json!({"source": "user"}))
        );
        assert_eq!(written.source(), TitleSource::User);
    }
}
