//! Searching a person's own threads (ADR 0068): the pure rules of `docs/api/thread-search-v1.md`.
//!
//! Words, keys, the query, when a document matches and the snippet are defined by the contract page
//! and computed **here, in Rust**; a database is only the index of the keys this module makes
//! ([`document_keys`]) and is asked for the keys of a query ([`SearchTerms::as_slice`]), so the
//! candidates it finds are the contract's whatever its collation or text-search configuration. The
//! web's mock server (`web/mock/search.ts`) is a second implementation of the same page, and both
//! read the vectors of `docs/api/examples/thread-search/`.
//!
//! No I/O and no async. Offsets are **UTF-16 code units** (as JavaScript counts them, and as the
//! contract says), never bytes and never code points, and a snippet's edges are word edges, so no
//! surrogate pair is ever split.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::event::{Event, EventBody, MessagePurpose};

/// The longest query, in Unicode scalar values, after it is trimmed.
pub const MAX_QUERY_LENGTH: usize = 200;
/// The most terms of a query that are used.
pub const MAX_TERMS: usize = 8;
/// A term of fewer code points is dropped.
pub const MIN_TERM: usize = 2;
/// The longest key in code points: a longer word is not indexed, a longer term is cut.
pub const MAX_KEY: usize = 64;
/// The most words of one document that are searched.
pub const MAX_WORDS: usize = 3000;
/// The most message hits one thread shows.
pub const MAX_MESSAGE_HITS: usize = 3;
/// The words in front of the first match that a snippet starts with, at most.
pub const CONTEXT_WORDS: usize = 6;
/// A snippet does not reach further back than this before the first match, in UTF-16 units.
pub const CONTEXT_UNITS: usize = 100;
/// A snippet is about this long, in UTF-16 units.
pub const WINDOW_UNITS: usize = 200;
/// Text left over at the very start or end of a document is kept up to this much, in UTF-16 units.
pub const EDGE_UNITS: usize = 20;

/// The version of the folding rules, stored with what a thread has indexed (`threads.search_fold`).
///
/// **Raise it whenever the key of any word could change**: a change to [`fold`], to [`WORD`]'s
/// pattern, to [`MAX_KEY`] or [`MAX_WORDS`], or a bump of a dependency whose tables the fold reads
/// (Rust's `to_lowercase`, `regex-syntax`'s `\p{L}`, `\p{M}`, `\p{N}`, and `unicode-normalization`'s
/// NFKD). A stored key made by the old rules and a query key made by the new ones would otherwise
/// differ without any error. **A failing vector of `fold.json` (or the unit test
/// `the_fold_is_pinned_raise_fold_version_when_it_moves`) after a dependency bump means: raise this
/// number**, then run `orchestrator search backfill` (a thread whose stored version is not this one is
/// unindexed: it is not found, and its next commit or the backfill indexes it again from the start).
pub const FOLD_VERSION: i32 = 1;
/// How many events of a log, at most, one commit catches the index up with besides its own: the rest of
/// an old log is left to the next commits and to `orchestrator search backfill`, so the first commit to
/// a long thread is not the one that indexes all of it under the thread's lock.
pub const COMMIT_CATCH_UP_EVENTS: i64 = 500;

/// `[\p{L}\p{M}\p{N}]+`: a word.
#[allow(clippy::expect_used)] // a constant pattern, proven by every test of this module
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{M}\p{N}]+").expect("the word pattern is valid"));
/// A mark.
#[allow(clippy::expect_used)] // a constant pattern, proven by every test of this module
static MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{M}").expect("the mark pattern is valid"));
/// What is neither a letter nor a number.
#[allow(clippy::expect_used)] // a constant pattern, proven by every test of this module
static NOT_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]").expect("the key pattern is valid"));
/// A key: letters and numbers, and nothing else.
#[allow(clippy::expect_used)] // a constant pattern, proven by every test of this module
static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}]+$").expect("the key pattern is valid"));

/// The key of one word: NFKD, no marks, lower case, `œ` `æ` `ß` spelled out, the final sigma made a
/// sigma, and nothing that is not a letter or a number. `""` when nothing is left. No length limit.
pub fn fold(word: &str) -> String {
    let decomposed: String = word.nfkd().collect();
    let unmarked = MARK.replace_all(&decomposed, "");
    let lower = unmarked
        .to_lowercase()
        .replace('œ', "oe")
        .replace('æ', "ae")
        .replace('ß', "ss")
        .replace('ς', "σ");
    NOT_KEY.replace_all(&lower, "").into_owned()
}

/// One word of a text, with where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// UTF-16 offset of its first unit.
    pub start: usize,
    /// UTF-16 offset after its last unit.
    pub end: usize,
    /// Its key; `None` when it has none (nothing is left of it, or it is longer than [`MAX_KEY`]).
    pub key: Option<String>,
}

/// The words of a text (the first [`MAX_WORDS`]) and whether there are more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Words {
    /// The words, in order.
    pub words: Vec<Word>,
    /// Whether the text has words after the ones listed.
    pub more: bool,
}

/// A word with its place in bytes too, for cutting the text.
struct Located {
    word: Word,
    byte_start: usize,
    byte_end: usize,
}

fn locate(text: &str) -> (Vec<Located>, bool) {
    let mut out = Vec::new();
    let mut more = false;
    let mut at_byte = 0;
    let mut at_unit = 0;
    for m in WORD.find_iter(text) {
        if out.len() == MAX_WORDS {
            more = true;
            break;
        }
        at_unit += text[at_byte..m.start()].encode_utf16().count();
        let len = m.as_str().encode_utf16().count();
        let key = fold(m.as_str());
        let key = (!key.is_empty() && key.chars().count() <= MAX_KEY).then_some(key);
        out.push(Located {
            word: Word {
                start: at_unit,
                end: at_unit + len,
                key,
            },
            byte_start: m.start(),
            byte_end: m.end(),
        });
        at_unit += len;
        at_byte = m.end();
    }
    (out, more)
}

/// The words of `text`, as the contract defines them.
pub fn words(text: &str) -> Words {
    let (located, more) = locate(text);
    Words {
        words: located.into_iter().map(|l| l.word).collect(),
        more,
    }
}

/// The distinct keys of a document's words, sorted: what an index stores for a title, a description
/// or a message.
pub fn document_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = words(text)
        .words
        .into_iter()
        .filter_map(|w| w.key)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Why a query is refused (the `code` of the 400).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    /// Nothing is left of the query after it is trimmed.
    #[error("the query is empty")]
    Empty,
    /// More than [`MAX_QUERY_LENGTH`] characters.
    #[error("the query is longer than {MAX_QUERY_LENGTH} characters")]
    TooLong,
}

impl QueryError {
    /// The `code` of the problem the API answers.
    pub fn code(self) -> &'static str {
        match self {
            QueryError::Empty => "query_empty",
            QueryError::TooLong => "query_too_long",
        }
    }
}

/// Keys that cannot be terms.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a term: {0}")]
pub struct NotATerm(pub &'static str);

/// The terms of a query: at most [`MAX_TERMS`] distinct keys of [`MIN_TERM`] to [`MAX_KEY`] code
/// points, each **only letters and numbers**. It is the one type that may reach an index: a store
/// builds its query out of these and nothing else, and no constructor accepts a quote, a colon or a
/// space, so a person's text can never be syntax.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchTerms(Vec<String>);

impl SearchTerms {
    /// The terms of a query, as the contract reads it.
    ///
    /// # Errors
    /// [`QueryError::Empty`] when nothing is left after trimming U+0020, U+0009, U+000A and U+000D;
    /// [`QueryError::TooLong`] for more than [`MAX_QUERY_LENGTH`] scalar values.
    pub fn parse(q: &str) -> Result<Self, QueryError> {
        let trimmed = q.trim_matches([' ', '\t', '\n', '\r']);
        if trimmed.is_empty() {
            return Err(QueryError::Empty);
        }
        if trimmed.chars().count() > MAX_QUERY_LENGTH {
            return Err(QueryError::TooLong);
        }
        let mut terms: Vec<String> = Vec::new();
        for m in WORD.find_iter(trimmed) {
            let key: String = fold(m.as_str()).chars().take(MAX_KEY).collect();
            if key.chars().count() < MIN_TERM || terms.contains(&key) {
                continue;
            }
            terms.push(key);
            if terms.len() == MAX_TERMS {
                break;
            }
        }
        Ok(SearchTerms(terms))
    }

    /// Terms from keys already made, checked: at most [`MAX_TERMS`], each of [`MIN_TERM`] to
    /// [`MAX_KEY`] code points and only letters and numbers.
    ///
    /// # Errors
    /// [`NotATerm`] says which rule a key breaks.
    pub fn from_keys(keys: Vec<String>) -> Result<Self, NotATerm> {
        if keys.len() > MAX_TERMS {
            return Err(NotATerm("too many terms"));
        }
        for key in &keys {
            let n = key.chars().count();
            if !(MIN_TERM..=MAX_KEY).contains(&n) {
                return Err(NotATerm("a term has a bad length"));
            }
            if !KEY.is_match(key) {
                return Err(NotATerm("a term is not made of letters and numbers"));
            }
        }
        Ok(SearchTerms(keys))
    }

    /// The terms, in the query's order.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Whether the query kept no term (nothing can match).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many terms.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether a word with this key starts with a term.
    fn starts(&self, key: &str) -> bool {
        self.0.iter().any(|t| key.starts_with(t.as_str()))
    }

    /// Whether a set of keys matches: every term is a prefix of one of them.
    pub fn matches_keys<K: AsRef<str>>(&self, keys: &[K]) -> bool {
        !self.0.is_empty()
            && self
                .0
                .iter()
                .all(|t| keys.iter().any(|k| k.as_ref().starts_with(t.as_str())))
    }

    /// Whether a text matches: every term is a prefix of the key of one of its words.
    pub fn matches(&self, text: &str) -> bool {
        self.matches_keys(&document_keys(text))
    }
}

/// A marked word of a snippet, in UTF-16 units of [`Snippet::text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Marked {
    /// Offset of its first unit.
    pub start: usize,
    /// Offset after its last unit.
    pub end: usize,
}

/// A piece of a document around its first match (`docs/api/thread-search-v1.md`, "The snippet").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    /// The text, whitespace flattened. Never markup.
    pub text: String,
    /// Something of the document comes before it.
    pub cut_start: bool,
    /// Something of the document comes after it.
    pub cut_end: bool,
    /// The words of `text` that start with a term, in order.
    pub matches: Vec<Marked>,
}

/// U+0020, U+0009, U+000A and U+000D runs become one space, and the ends are trimmed.
fn flatten(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r') {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

/// The snippet of `text` for `terms`; `None` when no word of it starts with a term.
pub fn snippet(text: &str, terms: &SearchTerms) -> Option<Snippet> {
    let flat = flatten(text);
    let (located, more) = locate(&flat);
    let is_hit = |l: &Located| l.word.key.as_deref().is_some_and(|k| terms.starts(k));
    let at = located.iter().position(is_hit)?;
    let hit_start = located[at].word.start;
    let first = (at.saturating_sub(CONTEXT_WORDS)..at)
        .find(|&j| located[j].word.start + CONTEXT_UNITS >= hit_start)
        .unwrap_or(at);
    let mut start = located[first].word.start;
    let mut start_byte = located[first].byte_start;
    let mut last = at;
    for (j, l) in located.iter().enumerate().skip(at + 1) {
        if l.word.end <= start + WINDOW_UNITS {
            last = j;
        } else {
            break;
        }
    }
    let mut end = located[last].word.end;
    let mut end_byte = located[last].byte_end;
    let length = flat.encode_utf16().count();
    let marked: Vec<Marked> = located[first..=last]
        .iter()
        .filter(|l| is_hit(l))
        .map(|l| Marked {
            start: l.word.start,
            end: l.word.end,
        })
        .collect();
    if first == 0 && start <= EDGE_UNITS {
        start = 0;
        start_byte = 0;
    }
    if last + 1 == located.len() && !more && length - end <= EDGE_UNITS {
        end = length;
        end_byte = flat.len();
    }
    Some(Snippet {
        text: flat[start_byte..end_byte].to_owned(),
        cut_start: start > 0,
        cut_end: end < length,
        matches: marked
            .into_iter()
            .map(|m| Marked {
                start: m.start - start,
                end: m.end - start,
            })
            .collect(),
    })
}

/// Who said a searched message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchRole {
    /// The person.
    Person,
    /// The agent: a final answer.
    Agent,
}

/// What a log event does to the index of messages (ADR 0068).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageChange<'a> {
    /// Who said it.
    pub role: SearchRole,
    /// The earlier messages of the thread with this id are replaced (and forgotten) by this one: a
    /// later final message with the same `messageId`.
    pub replaces: Option<&'a str>,
    /// The words searched; `None` when the event replaces and says nothing searched (working text).
    pub text: Option<&'a str>,
}

/// What `event` changes in the index of messages: a person's `user_message`; an `agent_message` that
/// is final (its text unless it is working text, and it replaces the earlier ones of its id). Nothing
/// else is searched: partial messages, reasoning, steps, tool results, surfaces, form answers, file
/// names.
pub fn message_change(event: &Event) -> Option<MessageChange<'_>> {
    match &event.body {
        EventBody::UserMessage(d) => Some(MessageChange {
            role: SearchRole::Person,
            replaces: None,
            text: Some(&d.text),
        }),
        EventBody::AgentMessage(d) if d.is_final => Some(MessageChange {
            role: SearchRole::Agent,
            replaces: Some(&d.message_id),
            text: (d.purpose != Some(MessagePurpose::Working)).then_some(d.text.as_str()),
        }),
        _ => None,
    }
}

/// A searched message of a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchedMessage {
    /// The event's `seq`.
    pub seq: i64,
    /// Who said it.
    pub role: SearchRole,
    /// The event's time.
    pub at: jiff::Timestamp,
    /// The words.
    pub text: String,
}

/// The messages of a log that are searched, in `seq` order: the log's [`message_change`]s with a later
/// final message replacing the earlier ones of its id.
pub fn searched_messages<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<SearchedMessage> {
    let mut out: Vec<(Option<String>, SearchedMessage)> = Vec::new();
    for event in events {
        let Some(change) = message_change(event) else {
            continue;
        };
        if let Some(id) = change.replaces {
            out.retain(|(held, _)| held.as_deref() != Some(id));
        }
        if let Some(text) = change.text {
            out.push((
                change.replaces.map(str::to_owned),
                SearchedMessage {
                    seq: event.seq,
                    role: change.role,
                    at: event.at,
                    text: text.to_owned(),
                },
            ));
        }
    }
    out.into_iter().map(|(_, m)| m).collect()
}

#[cfg(test)]
mod tests;
