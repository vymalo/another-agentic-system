//! The suggestions offered under a job's finished answer (ADR 0035, amended 2026-11-28): at most
//! three short next things the person could say, which **fill the message box and never send**
//! (ADR 0013, rule 7).
//!
//! The model is asked for them after a job's final answer (`done` or `blocked`), **at most once
//! per job**, from **the conversation's last exchange only** (the person's last message and the
//! agent's final answer), never the whole log. What comes back ([`clean_suggestions`]) is data: at
//! most [`MAX_SUGGESTIONS`] lines of at most [`MAX_SUGGESTION_CHARS`] characters each, and the log
//! keeps them as a `thread_suggested` event ([`ThreadSuggestedData`]), scoped to the job.
//!
//! The thread remembers which job asked in its job ledger ([`SuggestionsLedger`]). Unlike the
//! description, a suggestion is not the thread's and nothing a person writes replaces it: a fork
//! inherits none, and a later job asks again.

use serde::{Deserialize, Serialize};

use crate::event::{Event, EventBody};
use crate::title::trim_markdown_edges;

/// The most suggestions one job is given.
pub const MAX_SUGGESTIONS: usize = 3;

/// The most characters of one suggestion.
pub const MAX_SUGGESTION_CHARS: usize = 80;

/// `data` of a `thread_suggested`: the suggestions offered under the final answer of `job`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSuggestedData {
    /// The job whose end asked.
    pub job: u32,
    /// At most [`MAX_SUGGESTIONS`] suggestions, each at most [`MAX_SUGGESTION_CHARS`] characters.
    pub suggestions: Vec<String>,
}

/// The thread's memory of the job that last asked the model for suggestions
/// (`Job.suggestions`). It belongs to the job in the sense that matters: a job asks at most once,
/// and the next job asks again. A fork starts with none, because its jobs are its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SuggestionsLedger {
    /// The job that last asked the model: a job asks at most once.
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

impl SuggestionsLedger {
    /// A ledger that has asked nothing, which the log leaves out.
    pub fn is_empty(&self) -> bool {
        self.asked_job == 0 && !self.pending
    }

    /// A ledger that has asked `asked_job` for suggestions, whose answer is `pending` or has come.
    pub const fn of(asked_job: u32, pending: bool) -> Self {
        SuggestionsLedger { asked_job, pending }
    }

    /// The ledger a fork starts with: nothing. Suggestions are per job, and the fork's jobs are
    /// its own.
    pub fn inherited(self) -> Self {
        SuggestionsLedger::default()
    }

    /// Whether the model may be asked at the end of `job`: this job has not asked.
    pub fn may_ask(&self, job: u32) -> bool {
        self.asked_job != job
    }

    /// The model is asked at the end of `job`.
    pub(crate) fn asked(&mut self, job: u32) {
        self.asked_job = job;
        self.pending = true;
    }

    /// The ask of `job` is answered, with suggestions or without. `false` for an answer to an ask
    /// that is not the one in flight (a stale or repeated one), which the core ignores.
    pub(crate) fn answered(&mut self, job: u32) -> bool {
        let current = self.pending && self.asked_job == job;
        if current {
            self.pending = false;
        }
        current
    }
}

/// Why a list of suggestions cannot be logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SuggestionsError {
    /// More than [`MAX_SUGGESTIONS`] suggestions.
    #[error("more than {MAX_SUGGESTIONS} suggestions")]
    TooMany,
    /// A suggestion longer than [`MAX_SUGGESTION_CHARS`] characters.
    #[error("a suggestion is longer than {MAX_SUGGESTION_CHARS} characters")]
    TooLong,
    /// A control character: a suggestion is one line of text.
    #[error("a suggestion has a control character")]
    Control,
    /// No suggestion at all.
    #[error("no suggestion")]
    Empty,
}

/// The suggestions the core logs for what a model (or the dispatcher) answered: each trimmed, with
/// no empty one, at most [`MAX_SUGGESTIONS`] of at most [`MAX_SUGGESTION_CHARS`] characters.
///
/// # Errors
/// [`SuggestionsError`] for too many, a too long one, a control character or none at all.
pub fn check_suggestions(raw: &[String]) -> Result<Vec<String>, SuggestionsError> {
    let mut out = Vec::new();
    for suggestion in raw {
        let suggestion = suggestion.trim();
        if suggestion.is_empty() {
            continue;
        }
        if suggestion.chars().any(char::is_control) {
            return Err(SuggestionsError::Control);
        }
        if suggestion.chars().count() > MAX_SUGGESTION_CHARS {
            return Err(SuggestionsError::TooLong);
        }
        out.push(suggestion.to_owned());
    }
    if out.is_empty() {
        return Err(SuggestionsError::Empty);
    }
    if out.len() > MAX_SUGGESTIONS {
        return Err(SuggestionsError::TooMany);
    }
    Ok(out)
}

/// The suggestions a model's answer stands for: one per line, in order, each with the bullets,
/// numbering, quotes and Markdown markers a model likes to add taken off the edges, spaces
/// collapsed, at most [`MAX_SUGGESTION_CHARS`] characters (cut at a word), at most
/// [`MAX_SUGGESTIONS`] of them. A repeated suggestion is said once, and a line that says `NONE`
/// (in any case, with or without a final period) is no suggestion.
pub fn clean_suggestions(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines() {
        let line = strip_marker(line.trim());
        let kept: String = line
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        let suggestion = trim_markdown_edges(&kept)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if suggestion.is_empty()
            || suggestion
                .trim_end_matches('.')
                .eq_ignore_ascii_case("none")
        {
            continue;
        }
        let suggestion = cut_suggestion(&suggestion);
        if !suggestion.is_empty() && !out.contains(&suggestion) {
            out.push(suggestion);
        }
        if out.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    out
}

/// The line without a leading bullet or number (`-`, `*`, `•`, `1.`, `1)`, `1:`), and the space
/// after it.
fn strip_marker(line: &str) -> &str {
    let trimmed = line
        .trim_start_matches(['-', '*', '•', '·', '–'])
        .trim_start();
    let digits = trimmed.len()
        - trimmed
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .len();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if let Some(rest) = rest
            .strip_prefix('.')
            .or_else(|| rest.strip_prefix(')'))
            .or_else(|| rest.strip_prefix(':'))
        {
            return rest.trim_start();
        }
    }
    trimmed
}

/// `text` at most [`MAX_SUGGESTION_CHARS`] characters, cut at the last space that leaves room
/// (the whole word is kept), else in the middle of the word. There is no ellipsis: the cut is the
/// suggestion the person is offered, not a marked truncation.
fn cut_suggestion(text: &str) -> String {
    if text.chars().count() <= MAX_SUGGESTION_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(MAX_SUGGESTION_CHARS).collect();
    let cut = match head.rfind(' ') {
        Some(i) if i > 0 => &head[..i],
        _ => head.as_str(),
    };
    cut.trim_end().to_owned()
}

/// The conversation's **last exchange** for a suggestions request: the person's last message and
/// the agent's last words **of the same job**, in that order, each one line. The job is the one the
/// last `job_started` of the window began; the person's message is the window's last, which the
/// boundary's commit writes just before it (a redelivery's message was written earlier, and a job
/// that went on from a blocked state has no boundary of its own). The agent's words are taken only
/// from after both, so a job whose answer left no words is not given the answer of the job before
/// it. Empty when the window has no message of the person.
pub(crate) fn last_exchange(events: &[Event]) -> Vec<(&str, &str)> {
    let boundary = events
        .iter()
        .rposition(|e| matches!(e.body, EventBody::JobStarted(_)))
        .unwrap_or(0);
    let Some((person_at, person)) =
        events
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, e)| match &e.body {
                EventBody::UserMessage(m) if !m.text.trim().is_empty() => Some((i, m.text.trim())),
                _ => None,
            })
    else {
        return Vec::new();
    };
    let agent = events[person_at.max(boundary) + 1..]
        .iter()
        .filter_map(|e| crate::title::agent_words(&e.body))
        .map(str::trim)
        .filter(|words| !words.is_empty())
        .next_back();
    let mut out = vec![("user", person)];
    if let Some(agent) = agent {
        out.push(("agent", agent));
    }
    out
}

/// Whether the log has an exchange a suggestions request can be built from: a message of the person
/// of the job it names, and the agent's answer when there is one. The dispatcher asks nothing for a
/// log without one.
pub fn has_exchange(events: &[Event]) -> bool {
    !last_exchange(events).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_model_answers_is_a_short_list_of_one_line_suggestions() {
        assert_eq!(
            clean_suggestions("- Summarise the changes\n* Run the tests\n\n1. Open a pull request"),
            [
                "Summarise the changes",
                "Run the tests",
                "Open a pull request"
            ]
        );
        assert_eq!(clean_suggestions("\"What changed?\""), ["What changed?"]);
        assert_eq!(clean_suggestions("NONE"), Vec::<String>::new());
        assert_eq!(clean_suggestions("none."), Vec::<String>::new());
        assert_eq!(clean_suggestions("  \n \n"), Vec::<String>::new());
    }

    #[test]
    fn at_most_three_and_each_at_most_the_limit() {
        let many = (1..=5)
            .map(|n| format!("Suggestion {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(clean_suggestions(&many).len(), MAX_SUGGESTIONS);
        let long = "word ".repeat(40);
        let got = clean_suggestions(&long);
        assert_eq!(got.len(), 1);
        assert!(got[0].chars().count() <= MAX_SUGGESTION_CHARS, "{}", got[0]);
        assert!(got[0].ends_with("word"), "{}", got[0]);
    }

    #[test]
    fn a_repeated_suggestion_is_said_once() {
        assert_eq!(
            clean_suggestions("Run the tests\nRun the tests\n- Run the tests"),
            ["Run the tests"]
        );
    }

    #[test]
    fn the_core_logs_only_a_bounded_list() {
        assert_eq!(
            check_suggestions(&["  Run the tests  ".to_owned()]).as_deref(),
            Ok(["Run the tests".to_owned()].as_slice())
        );
        assert_eq!(check_suggestions(&[]), Err(SuggestionsError::Empty));
        assert_eq!(
            check_suggestions(&[" ".to_owned()]),
            Err(SuggestionsError::Empty)
        );
        assert_eq!(
            check_suggestions(&["two\nlines".to_owned()]),
            Err(SuggestionsError::Control)
        );
        assert_eq!(
            check_suggestions(&["x".repeat(MAX_SUGGESTION_CHARS + 1)]),
            Err(SuggestionsError::TooLong)
        );
        let four: Vec<String> = (1..=4).map(|n| format!("S{n}")).collect();
        assert_eq!(check_suggestions(&four), Err(SuggestionsError::TooMany));
        let three: Vec<String> = (1..=3).map(|n| format!("S{n}")).collect();
        assert!(check_suggestions(&three).is_ok());
    }

    #[test]
    fn the_ledger_of_a_thread_that_asked_nothing_says_nothing() {
        let ledger = SuggestionsLedger::default();
        assert!(ledger.is_empty());
        assert_eq!(
            serde_json::to_value(ledger).ok(),
            Some(serde_json::json!({}))
        );
        let mut asked = SuggestionsLedger::default();
        asked.asked(3);
        assert_eq!(
            serde_json::to_value(asked).ok(),
            Some(serde_json::json!({"askedJob": 3, "pending": true}))
        );
    }

    #[test]
    fn a_job_asks_once_and_its_answer_counts_once() {
        let mut ledger = SuggestionsLedger::default();
        assert!(ledger.may_ask(1));
        ledger.asked(1);
        assert!(!ledger.may_ask(1), "once per job");
        assert!(ledger.may_ask(2), "a later job asks again");
        assert!(
            !ledger.answered(2),
            "an answer to another job's ask is stale"
        );
        assert!(ledger.answered(1));
        assert!(!ledger.answered(1), "a repeated answer is stale");
    }

    #[test]
    fn a_fork_inherits_no_ask() {
        let mut ledger = SuggestionsLedger::default();
        ledger.asked(4);
        assert_eq!(ledger.inherited(), SuggestionsLedger::default());
    }
}
