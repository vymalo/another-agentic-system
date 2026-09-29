//! The `vymalo.*` vocabulary of `docs/api/agui.md`: activity types, metadata keys, error codes.

use orch_agui_proto::Metadata;
use orch_core::{Actor, AgentStatus};
use serde_json::{Value, json};

/// Activity type of a status line (`{status, detail?}`).
pub const ACTIVITY_STATUS: &str = "vymalo.status";
/// Activity type of an artifact (`{name, mimeType?, uri?, text?}`).
pub const ACTIVITY_ARTIFACT: &str = "vymalo.artifact";
/// Activity type of an error (`{message, retryable}`).
pub const ACTIVITY_ERROR: &str = "vymalo.error";

/// Metadata key naming who produced an event (`{type, name, revision?}`).
pub const ACTOR_KEY: &str = "vymalo.actor";
/// Metadata key of a `RUN_ERROR` carrying the problem (`{type, title, detail?}`).
pub const PROBLEM_KEY: &str = "vymalo.problem";

/// `RUN_ERROR.code` for an agent that reported `failed` (or `rejected`).
pub const CODE_AGENT_FAILED: &str = "agent_failed";
/// `RUN_ERROR.code` for a delegation that could not be delivered.
pub const CODE_DELIVERY_FAILED: &str = "delivery_failed";

/// The release-channels extension URI (ADR 0008): the key of `forwardedProps` that selects a
/// release when a thread is created.
pub const RELEASE_CHANNELS_URI: &str =
    "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

/// `{"vymalo.actor": {type, name, revision?}}`.
pub(crate) fn actor_metadata(actor: &Actor) -> Metadata {
    let mut metadata = Metadata::new();
    // `Actor` is plain strings and an enum: it always serialises.
    metadata.insert(
        ACTOR_KEY.to_owned(),
        serde_json::to_value(actor).unwrap_or(Value::Null),
    );
    metadata
}

/// The `vymalo.status` content.
pub(crate) fn status_content(status: AgentStatus, detail: Option<&str>) -> Metadata {
    let mut content = Metadata::new();
    content.insert(
        "status".to_owned(),
        serde_json::to_value(status).unwrap_or(Value::Null),
    );
    if let Some(detail) = detail {
        content.insert("detail".to_owned(), Value::from(detail));
    }
    content
}

/// `{"vymalo.problem": {type, title, detail?}}`, in the shape of the API's RFC 9457 problems.
pub(crate) fn problem_metadata(title: &str, detail: &str) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert(
        PROBLEM_KEY.to_owned(),
        json!({"type": "about:blank", "title": title, "detail": detail}),
    );
    metadata
}

/// The schema of an interrupt's answer: `{text}`.
pub(crate) fn response_schema() -> Metadata {
    let mut schema = Metadata::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("required".to_owned(), json!(["text"]));
    schema.insert("properties".to_owned(), json!({"text": {"type": "string"}}));
    schema
}
