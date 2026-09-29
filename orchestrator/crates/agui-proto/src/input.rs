//! `RunAgentInput`: the one message from a consumer to a producer.
//!
//! The typed view is lenient on the way in, as the processing model requires: members the
//! schema does not declare are dropped, not rejected. [`RunAgentInput::parse`] says which were
//! dropped so the caller can warn. Malformed input (a missing required member, a wrong type,
//! an unknown message role) is an [`InputError`], to be rejected before the stream starts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{RunId, ThreadId};
use crate::message::{Context, Message, Tool};
use crate::run::ResumeEntry;

/// A request to run an agent. Also echoed back as `RUN_STARTED.input`.
///
/// `tools`, `context` and `resume` keep the distinction between absent and empty so that a
/// round trip is exact; read them through the accessors, which treat both as empty (the
/// protocol does).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAgentInput {
    /// The conversation. The application mints it.
    pub thread_id: ThreadId,
    /// This run. Never reused on a thread.
    pub run_id: RunId,
    /// The `MAJOR.MINOR` protocol version the consumer speaks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    /// The run that spawned this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// The state the run starts from. Any non-null JSON value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    /// The complete history the consumer is showing.
    pub messages: Vec<Message>,
    /// Frontend tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    /// Ambient context for the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<Context>>,
    /// Passed through untouched; intermediaries must not alter it. Any non-null JSON value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_props: Option<Value>,
    /// Answers to the interrupts of the previous run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Vec<ResumeEntry>>,
}

/// The input was malformed: reject the request before `RUN_STARTED`.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// The bytes are not JSON.
    #[error("run input is not valid JSON")]
    NotJson(#[source] serde_json::Error),
    /// The JSON does not have the shape of a `RunAgentInput`.
    #[error("run input is malformed")]
    Malformed(#[source] serde_json::Error),
}

/// A parsed input and the members its typed view does not carry.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedInput {
    /// The typed view.
    pub input: RunAgentInput,
    /// JSON paths (for example `messages[0].extra`) of members that were present but are not
    /// part of the typed view: unknown members, and optional members sent as `null`. The
    /// caller should log a warning; the request is still served.
    pub dropped: Vec<String>,
}

impl RunAgentInput {
    /// Parses request bytes, dropping unknown members.
    pub fn parse(bytes: &[u8]) -> Result<ParsedInput, InputError> {
        let value: Value = serde_json::from_slice(bytes).map_err(InputError::NotJson)?;
        Self::from_value(value)
    }

    /// Reads a JSON value, dropping unknown members.
    pub fn from_value(value: Value) -> Result<ParsedInput, InputError> {
        let input: RunAgentInput =
            serde_json::from_value(value.clone()).map_err(InputError::Malformed)?;
        // The typed view serialises exactly what it carries; whatever the request had beyond
        // that was dropped.
        let kept = serde_json::to_value(&input).map_err(InputError::Malformed)?;
        let mut dropped = Vec::new();
        collect_dropped(&value, &kept, String::new(), &mut dropped);
        Ok(ParsedInput { input, dropped })
    }

    /// The frontend tools; empty when absent.
    pub fn tools(&self) -> &[Tool] {
        self.tools.as_deref().unwrap_or_default()
    }

    /// The context entries; empty when absent.
    pub fn context(&self) -> &[Context] {
        self.context.as_deref().unwrap_or_default()
    }

    /// The resume entries; empty when absent.
    pub fn resume(&self) -> &[ResumeEntry] {
        self.resume.as_deref().unwrap_or_default()
    }
}

fn collect_dropped(original: &Value, kept: &Value, path: String, out: &mut Vec<String>) {
    match (original, kept) {
        (Value::Object(orig), Value::Object(kept)) => {
            for (key, value) in orig {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match kept.get(key) {
                    Some(kept_value) => collect_dropped(value, kept_value, child, out),
                    None => out.push(child),
                }
            }
        }
        (Value::Array(orig), Value::Array(kept)) => {
            for (index, (value, kept_value)) in orig.iter().zip(kept).enumerate() {
                collect_dropped(value, kept_value, format!("{path}[{index}]"), out);
            }
        }
        _ => {}
    }
}
