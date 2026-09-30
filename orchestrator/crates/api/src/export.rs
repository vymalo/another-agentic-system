//! The thread export document (`GET /api/threads/{id}/export`): one JSON file that holds what a
//! developer needs to see what happened in a chat, without access to the deployment.
//!
//! The document is versioned. `version` moves only when a member changes meaning or goes away;
//! adding a member does not move it, so a reader ignores what it does not know.

use orch_app::ThreadExport;
use orch_core::Event;
use serde_json::{Value, json};

/// The `format` member: what kind of file this is.
pub const FORMAT: &str = "another-agentic-system/thread-export";
/// The `version` member: the document's shape, from 1.
pub const VERSION: u32 = 1;

/// The document for `export`.
///
/// * `thread`: the contract `Thread` (what `GET /api/threads/{id}` answers), so a reader that
///   knows the API knows this member.
/// * `job`: the whole job ledger, which `Thread.job` only summarises (and omits without a
///   gate): the gate policy, the attempt, the verification counter, the pushed commit, every
///   result of the current attempt and any hold. A job without a gate is `{}` apart from the
///   defaults, exactly as the store keeps it.
/// * `binding`: the A2A agent, context and task the thread is bound to.
/// * `events`: the append-only log in order, each exactly as the contract `Event` and the store
///   serialise it. Every card and line of the chat is derived from it.
/// * `eventsTruncated`: `true` when the log was longer than the export reads.
///
/// The owner's identity is not a member. It is in the log, as the `actor.name` of the owner's
/// messages.
pub fn document(export: &ThreadExport) -> Value {
    json!({
        "format": FORMAT,
        "version": VERSION,
        "exportedAt": export.exported_at,
        "thread": export.thread,
        "job": export.thread.job,
        "binding": export.binding.as_ref().map(|b| json!({
            "agentId": b.agent_id,
            "contextId": b.context_id,
            "taskId": b.task_id,
            "taskState": b.task_state,
            "revision": b.revision,
        })),
        "events": export.events.iter().map(event).collect::<Vec<_>>(),
        "eventsTruncated": export.truncated,
    })
}

fn event(e: &Event) -> Value {
    // `Event` serialises as the contract says; it cannot fail (plain strings, numbers and enums).
    serde_json::to_value(e).unwrap_or(Value::Null)
}
