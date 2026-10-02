//! What can be wrong with a configuration file, as a key path and a kind.
//!
//! **No error carries a value** (ADR 0034): not a value of the file, of the environment or of a
//! secret file. A message names the key path, what is wrong and what is allowed. The kinds hold
//! only what the program knows without the file (a bound from the schema, a variable name or a
//! path taken from a reference, a line and a column), and the library messages that quote their
//! input are never passed through.

use std::fmt;

/// One thing wrong with the configuration: the key it is about and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConfigError {
    /// The key path, such as `database.url` or `server.surfaces[1]`; empty for the document.
    pub path: String,
    /// What is wrong.
    pub kind: ErrorKind,
}

impl ConfigError {
    pub(crate) fn new(path: impl Into<String>, kind: ErrorKind) -> Self {
        ConfigError {
            path: path.into(),
            kind,
        }
    }

    /// A rule that is broken, with a reason that is written in the program, never taken from the
    /// file.
    pub(crate) fn invalid(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(path, ErrorKind::Invalid(reason.into()))
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "{}: {}", self.path, self.kind)
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.kind)
    }
}

/// What is wrong.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, thiserror::Error)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The text is not YAML (reported alone: nothing after it can be read).
    #[error("the YAML cannot be read (line {line}, column {column})")]
    Syntax {
        /// 1-based line, 0 when the parser did not say.
        line: usize,
        /// 1-based column, 0 when the parser did not say.
        column: usize,
    },
    /// A key is repeated in one mapping: never "the last one wins".
    #[error("a key is repeated in one mapping{detail}")]
    DuplicateKey {
        /// Where, and which key when it is a plain name, written for the message.
        detail: String,
    },
    /// A YAML tag (`!something`).
    #[error("a YAML tag is not allowed")]
    Tag,
    /// A mapping key that is not a string.
    #[error("a mapping key must be a string")]
    NonStringKey,
    /// A number the file's tree cannot hold (not finite).
    #[error("a number must be finite")]
    BadNumber,
    /// The document is not a mapping.
    #[error("the configuration must be a mapping, starting with `version: 1`")]
    NotAMapping,
    /// `version` is missing or is not `1`.
    #[error("this build reads version 1 (write `version: 1`)")]
    Version,
    /// A key this format does not have.
    #[error("unknown key")]
    Unknown,
    /// A key that is reserved for a later change.
    #[error("reserved for {by}, which is not built yet; remove the key")]
    Reserved {
        /// The change that brings it, for example `PR S18 (ADR 0035)`.
        by: &'static str,
    },
    /// A required key is missing.
    #[error("required key is missing")]
    Missing,
    /// A value of the wrong JSON type.
    #[error("expected {expected}")]
    WrongType {
        /// The type that is wanted, such as `string` or `integer`.
        expected: String,
    },
    /// A value that is not one of the allowed ones.
    #[error("not an allowed value (allowed: {allowed})")]
    NotAllowed {
        /// The allowed values, comma separated.
        allowed: String,
    },
    /// A plain string (or anything else) where a secret goes.
    #[error("a secret is a reference: `{{ env: NAME }}` or `{{ file: PATH }}`")]
    NotASecretRef,
    /// A number below its minimum.
    #[error("must be at least {min}")]
    TooSmall {
        /// The lowest value.
        min: String,
    },
    /// A number above its maximum.
    #[error("must be at most {max}")]
    TooLarge {
        /// The highest value.
        max: String,
    },
    /// A list with too few items.
    #[error("needs at least {min} item(s)")]
    TooFewItems {
        /// The fewest items.
        min: u64,
    },
    /// A list with too many items.
    #[error("takes at most {max} item(s)")]
    TooManyItems {
        /// The most items.
        max: u64,
    },
    /// A text that is too short.
    #[error("must not be empty")]
    Empty,
    /// Any other rule of the schema.
    #[error("does not satisfy the schema (rule `{keyword}`)")]
    Schema {
        /// The schema keyword that failed.
        keyword: String,
    },
    /// A rule that is not about the shape: a range between keys, a URL, a cross-key rule. The
    /// reason is written in the program.
    #[error("{0}")]
    Invalid(String),
    /// A secret reference that cannot be resolved.
    #[error("{0}")]
    Unresolved(String),
}

/// The part of a message that says where a YAML error is.
pub(crate) fn where_detail(name: Option<&str>, line: usize, column: usize) -> String {
    let mut detail = String::new();
    if let Some(name) = name {
        detail.push_str(&format!(" (`{name}`)"));
    }
    if line > 0 {
        detail.push_str(&format!(" (line {line}, column {column})"));
    }
    detail
}

/// `errors` as the lines an operator reads, one per error.
pub fn render(errors: &[ConfigError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
