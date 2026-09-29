//! RFC 6902 JSON Patch, as carried by `STATE_DELTA` and `ACTIVITY_DELTA`.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// A JSON Patch document: an ordered list of operations. Empty is a valid no-op.
pub type JsonPatch = Vec<JsonPatchOperation>;

/// A well-formed RFC 6901 JSON Pointer: empty, or slash-prefixed tokens where `~` is escaped
/// as `~0` and `/` as `~1`.
///
/// Structural validity only: a pointer may still name a path that does not exist in the
/// document it is applied to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct JsonPointer(String);

/// The text is not an RFC 6901 JSON Pointer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a JSON Pointer (RFC 6901): {0:?}")]
pub struct InvalidJsonPointer(String);

impl JsonPointer {
    /// The pointer to the whole document (the empty string).
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Checks `text` against RFC 6901.
    pub fn new(text: impl Into<String>) -> Result<Self, InvalidJsonPointer> {
        let text = text.into();
        if is_json_pointer(&text) {
            Ok(Self(text))
        } else {
            Err(InvalidJsonPointer(text))
        }
    }

    /// The pointer text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_json_pointer(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if !text.starts_with('/') {
        return false;
    }
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

impl fmt::Display for JsonPointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for JsonPointer {
    type Error = InvalidJsonPointer;
    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::new(text)
    }
}

impl TryFrom<&str> for JsonPointer {
    type Error = InvalidJsonPointer;
    fn try_from(text: &str) -> Result<Self, Self::Error> {
        Self::new(text)
    }
}

impl<'de> Deserialize<'de> for JsonPointer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(text).map_err(serde::de::Error::custom)
    }
}

/// One RFC 6902 operation. Closed over the six operations the RFC defines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum JsonPatchOperation {
    /// Inserts `value` at `path`.
    Add {
        /// Where to insert.
        path: JsonPointer,
        /// Any JSON value, `null` included.
        value: Value,
    },
    /// Removes the value at `path`.
    Remove {
        /// What to remove.
        path: JsonPointer,
    },
    /// Replaces the value at `path`.
    Replace {
        /// What to replace.
        path: JsonPointer,
        /// Any JSON value, `null` included.
        value: Value,
    },
    /// Moves the value at `from` to `path`.
    Move {
        /// Source.
        from: JsonPointer,
        /// Destination.
        path: JsonPointer,
    },
    /// Copies the value at `from` to `path`.
    Copy {
        /// Source.
        from: JsonPointer,
        /// Destination.
        path: JsonPointer,
    },
    /// Fails the patch unless the value at `path` equals `value`.
    Test {
        /// What to test.
        path: JsonPointer,
        /// Any JSON value, `null` included.
        value: Value,
    },
}
