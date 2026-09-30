//! The thread export document (`GET /api/threads/{id}/export`): one JSON file that holds what a
//! developer needs to see what happened in a chat, without access to the deployment.
//!
//! The document is versioned. `version` moves only when a member changes meaning or goes away;
//! adding a member does not move it, so a reader ignores what it does not know.

use orch_app::ThreadExport;
use orch_core::{AgentId, AgentTaskState, Event, Job, ThreadRecord, Timestamp};
use serde::Serialize;

/// The `format` member: what kind of file this is.
pub const FORMAT: &str = "another-agentic-system/thread-export";
/// The `version` member: the document's shape, from 1.
pub const VERSION: u32 = 1;

/// The document for an export, borrowed from it: serialising it writes the file straight from the
/// thread and its events, with no copy of the log in between.
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
/// * `eventsTruncated`: `true` when the log was longer than the export reads (it reads the head
///   of the log, up to a count of events and a number of bytes).
///
/// The owner's identity is not a member. It is in the log, as the `actor.name` of the owner's
/// messages.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document<'a> {
    format: &'static str,
    version: u32,
    exported_at: &'a Timestamp,
    thread: &'a ThreadRecord,
    job: &'a Job,
    binding: Option<Binding<'a>>,
    events: &'a [Event],
    events_truncated: bool,
}

/// The `binding` member.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Binding<'a> {
    agent_id: &'a AgentId,
    context_id: &'a str,
    task_id: Option<&'a str>,
    task_state: Option<AgentTaskState>,
    revision: Option<&'a str>,
}

/// The document for `export`.
pub fn document(export: &ThreadExport) -> Document<'_> {
    Document {
        format: FORMAT,
        version: VERSION,
        exported_at: &export.exported_at,
        thread: &export.thread,
        job: &export.thread.job,
        binding: export.binding.as_ref().map(|b| Binding {
            agent_id: &b.agent_id,
            context_id: &b.context_id,
            task_id: b.task_id.as_deref(),
            task_state: b.task_state,
            revision: b.revision.as_deref(),
        }),
        events: &export.events,
        events_truncated: export.truncated,
    }
}
