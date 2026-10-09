//! Pure mapping from A2A 1.0 values to protocol-neutral [`AgentEnvelope`]s: no I/O, no async, no
//! HTTP client (only the A2A value types of `a2a-lf`). The A2A client adapter (`orch-agent-a2a`)
//! feeds it the stream; a future in-process A2A host can use the same keys.
//!
//! Entry points: [`StreamMapper`] for a live stream and [`snapshot`] for a polled task.
//!
//! Idempotency keys (the dispatcher stores them so replays never duplicate chat events):
//!
//! | A2A value | key |
//! |---|---|
//! | artifact `X` of task `T` | `Task("a2a:T:artifact:X")` |
//! | agent message `M` | `Task("a2a:msg:M")` |
//! | status carrying message `M` | `Task("a2a:T:status-msg:M")` |
//! | status without a message | `Turn("T:status:<state>")` (the dispatcher prefixes the outbox row) |
//!
//! Files (ADR 0032). A `raw` part of an artifact is a file: one envelope per such part,
//! `AgentUpdate::File { name, media_type, filename, bytes }` (`name` is the artifact's, the type and the
//! file name are the part's own, blank text is none), under its own key. The artifact's other parts
//! make the artifact as before, and an artifact whose parts are all files has no artifact envelope. A
//! `raw` part of a message is ignored, as it was. A `url` part stays the artifact's `uri`: this crate
//! does no I/O, so fetching one from an allowed host is the A2A adapter's.
//!
//! | File in | key |
//! |---|---|
//! | artifact `X` of task `T`, part `i` (a `raw` part) | `Task("a2a:T:artifact:X:file:i")` |
//!
//! Steps (ADR 0025, `steps/v1`). A status whose state is `working` and whose message carries a
//! valid entry under the extension's URI (`docs/api/steps-v1.md`) is a step, not a status: it maps
//! to one envelope, `AgentUpdate::Step`, with `task_state: Working`, and the status text (the label
//! for a client that ignores the extension) is not logged. The ids are made unique within the
//! thread by prefixing the task id (`<task>/<id>`, and the parent likewise). An entry that does not
//! validate, or on a status that is not `working`, is ignored: the status is read as plain A2A (fail
//! closed). The response is read as data whether or not the request activated the extension, as A2UI
//! is, so a stream, a resubscribe and a poll map to the same keys.
//!
//! | Step in | key |
//! |---|---|
//! | status message `M` of task `T`, step `S` | `Task("a2a:T:step:S:M")` |
//! | status message without an id | `Turn("T:step:S:<step state>")` |
//!
//! Streamed text (ADR 0027, `text-stream/v1`, `docs/api/text-stream-v1.md`). An artifact update
//! whose artifact carries a valid entry under the extension's URI in its metadata (`{offset}`, a
//! UTF-8 byte offset, with a stream id of 1 to 128 printable bytes as `artifactId` and exactly one
//! text part) is a **chunk of a reply being written**: one envelope with `live: Some(LiveChunk)`
//! and no update, never assembled, never an artifact, never in the log. An entry that does not
//! validate leaves the update what it was, a plain A2A artifact chunk (fail closed). A `Task`
//! never holds one (chunks are transient), so a snapshot skips an artifact that carries the entry.
//! The agent states the whole text once, in a status message whose metadata names the stream
//! (`{streamId}`) and whose text is the whole reply: that maps to one
//! `AgentUpdate::Message { message_id: <stream id>, text, is_final: true, purpose }` first (ADR
//! 0031: `purpose` is `working` for a `working` status and `answer` for `completed`,
//! `input_required` and `auth_required`; a plain A2A `Message`, and the words of any other
//! status, are not marked), then the status
//! (a `working` one without a `detail`: its words are the message; a turn-ending one keeps its
//! `detail`, which the interrupt and the verifier read, and the projection says words equal to the
//! last final message only once). A status that is a step is a step and its text is its label: the
//! marker is ignored.
//!
//! | Streamed text in | key |
//! |---|---|
//! | chunk of stream `S`, byte offset `o`, of task `T` | `Turn("T:live:S:o")` (never applied) |
//! | the whole text of stream `S` | `Task("a2a:msg:S")` |
//!
//! Reasoning (ADR 0044, `kind: "reasoning"` of `text-stream/v1`). A chunk whose entry says
//! `"kind": "reasoning"` is a chunk like a reply's, with its own stream id, and its envelope's
//! [`LiveChunk`] says [`LiveKind::Reasoning`]; a `kind` that is something else (a kind this reader does
//! not know) is neither a reply nor an artifact, and the chunk is ignored. **The mapper collects a
//! reasoning's chunks** (the stream's beginning must have passed through it, at most
//! `MAX_REASONING_BYTES` kept, an overlap trimmed) and, when the last chunk ends the stream, maps it to
//! one more envelope after the live one: `AgentUpdate::Reasoning { message_id: <stream id>, text,
//! truncated }`, where `truncated` says that a piece was lost, the agent gave up or the bound was
//! reached. Nothing states a reasoning whole on the wire, and a stream whose beginning this mapper did
//! not see (a resubscribe) is relayed and not logged.
//!
//! | Reasoning in | key |
//! |---|---|
//! | chunk of the reasoning stream `R`, byte offset `o`, of task `T` | `Turn("T:live:R:o")` (never applied) |
//! | the whole reasoning `R`, when its stream ends | `Task("a2a:T:reasoning:R")` |
//!
//! An answer given as an artifact (ADR 0031, amendment of 2026-10-07). An agent that does not
//! state its words on a status or in a `Message` (kagent's runtimes: the reply is the words of
//! `working` statuses, and the last model event's text, once, as **one unnamed artifact**, then
//! `completed` with no message) has said nothing the log would show as its answer. The rule: the
//! text of an artifact that **names nothing** (no `name`, or a blank one: an agent that names an
//! artifact says it is a deliverable) and **is only text** (every part text: a data, `raw` or `url`
//! part makes it the artifact it was) is *kept* until the turn ends instead of being said at once.
//! Appended chunks of it are one text, concatenated as written. When a **`completed`** status then
//! arrives that has **no words of its own** and the stream **has stated no message** (no streamed
//! text, no plain `Message`: the agent says its words that way, and then the text is an
//! artifact), what was kept is said as **one** `AgentUpdate::Message { purpose: Some(Answer),
//! is_final: true }` (several artifacts' texts in order, a blank line between them) **before** the
//! status, under the message id `<task>:artifact:<first artifact id>` and the key
//! `Task("a2a:msg:<message id>")`, and is not also an artifact. Any other end of the turn
//! (`failed`, `canceled`, `rejected`, `input_required`, `auth_required`), a completion with words
//! and a `Message` frame give what was kept back as the artifacts they are, before that event;
//! `working` and `submitted` statuses leave it kept. A stream that ends with text kept says nothing
//! (as for an artifact held back): the poll has it. A snapshot applies the same rule to a task
//! that is `completed` with no words of its own, under the same key, and cannot know what a
//! stream said before it: an agent that states its words says them on the status, so it is not
//! affected, and one that is neither named nor stated nothing to be told apart by.
//!
//! | Answer in | key |
//! |---|---|
//! | unnamed text artifact(s) of task `T`, the first `X` | `Task("a2a:msg:T:artifact:X")` |
//!
//! Token usage (ADR 0056, `usage/v1`, `docs/api/usage-v1.md`). A `working` status update whose
//! **event's** `metadata` carries an entry under the extension's URI is a **call report**: one
//! envelope, `AgentUpdate::Usage(UsageUpdate::Call)` with `task_state: Working`, read through
//! [`orch_core::UsageCall::parse`] (a `stepId` is made unique within the thread like a step's id,
//! `<task>/<stepId>`). A status update that carries one and **no message** is the report and nothing
//! else: no status envelope, so no empty status and no empty text. A status update that ends or
//! pauses the turn (terminal, `input-required`, `auth-required`) and carries the entry, and a `Task`
//! in such a state whose own `metadata` carries it, give the task's **totals**,
//! `AgentUpdate::Usage(UsageUpdate::Total)`, before the status. An entry that does not pass the
//! door is `AgentUpdate::UsageRejected(why)`: never logged, counted by the application. Like steps,
//! the response is read as data whether or not the request activated the extension.
//! [`wants_usage_totals`] names a status update that ends the turn without its totals, and
//! [`usage_totals`] reads them from the task a `GetTask` returns, for the adapter's one read.
//!
//! | Usage in | key |
//! |---|---|
//! | the call report `C` of task `T` | `Task("a2a:T:usage:C")` (a replay or a poll collapses) |
//! | the totals of task `T` when it reached `<state>` | `Turn("T:usage-total:<state>")` |
//! | a report that does not pass the door | `Turn("T:usage-rejected")` (never applied) |
//!
//! A snapshot (`Task`, from `GetTask`, `CancelTask` or the first frame of `SubscribeToTask`)
//! maps to the same keys as the live stream, so a poll after a crash and the live events it
//! replaces collapse into one.
//!
//! A2UI (ADR 0013). A `Data` part whose `mediaType` (A2A 1.0) or `metadata.mimeType` (the A2UI
//! extension's own spelling) is `application/a2ui+json` carries an array of A2UI messages, in an
//! agent message, a status message or an artifact. It never becomes text: each such part maps to
//! one envelope, `AgentUpdate::Ui` when its array passes the envelope check
//! ([`orch_core::check_operations`]: an array, within the size cap, every message a known
//! version and operation) and `AgentUpdate::UiRejected` when it does not. Nothing unchecked is
//! passed on. The envelopes of a status message come before the status, so a surface that
//! accompanies `input-required` is recorded while the run is still open.
//!
//! | A2UI part in | key |
//! |---|---|
//! | artifact `X` of task `T`, part `i` | `Task("a2a:T:artifact:X:ui:i")` |
//! | agent message `M`, part `i` | `Task("a2a:msg:M:ui:i")` |
//! | status message `M` of task `T`, part `i` | `Task("a2a:T:status-msg:M:ui:i")` |
//! | status message without an id | `Turn("T:status-ui:<state>:i")` |
//!
//! Artifacts and chunking. On the wire (ProtoJSON) `append: false` and `lastChunk: false` are
//! indistinguishable from "unset", so the first chunk of a chunked artifact looks exactly like a
//! whole artifact. An artifact that is not marked `lastChunk: true` is therefore held back until
//! the next event proves no continuation follows (any other event, or `lastChunk: true`), and
//! is then emitted once, assembled, under its single key. A stream that ends with one still held
//! back drops it: the dispatcher then polls `GetTask`, whose snapshot has it whole, under the
//! same key. The cost is that such an artifact appears when the agent's next event does (a
//! status update normally follows immediately).
//!
//! Known limits, by design: `Task.history` is not replayed (a live stream reports agent
//! messages as `Message` events and status messages as status detail; history mixes both and
//! cannot be mapped back to the same keys), and a snapshot taken while an artifact is still
//! being appended chunk by chunk contains only the chunks so far.

use std::collections::HashMap;

use a2a::{
    Message, Part, PartContent, Role, StreamResponse, Task, TaskArtifactUpdateEvent, TaskState,
    TaskStatus,
};
use orch_core::{
    A2UI_MEDIA_TYPE, AgentTaskState, AgentUpdate, LiveChunk, LiveEnd, LiveKind,
    MAX_REASONING_BYTES, MessagePurpose, STEPS_EXTENSION, StepKind, StepOutput, StepReport,
    StepState, TEXT_STREAM_EXTENSION, USAGE_EXTENSION, UsageCall, UsageTotals, UsageUpdate,
    check_operations,
};
use orch_ports::{AgentEnvelope, AgentError, IdemKey, TaskSnapshot};
use serde_json::Value;

/// URI of the release-channels v1 extension (ADR 0008). Here because the revision an agent echoes
/// in event metadata is read under this key; `orch-agent-a2a` re-exports it for the request side.
pub const RELEASE_CHANNELS_URI: &str =
    "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

type Metadata = Option<HashMap<String, Value>>;

/// The neutral state of an A2A state; `None` for `Unspecified`.
fn state_of(state: &TaskState) -> Option<AgentTaskState> {
    match state {
        TaskState::Unspecified => None,
        TaskState::Submitted => Some(AgentTaskState::Submitted),
        TaskState::Working => Some(AgentTaskState::Working),
        TaskState::Completed => Some(AgentTaskState::Completed),
        TaskState::Failed => Some(AgentTaskState::Failed),
        TaskState::Canceled => Some(AgentTaskState::Canceled),
        TaskState::InputRequired => Some(AgentTaskState::InputRequired),
        TaskState::Rejected => Some(AgentTaskState::Rejected),
        TaskState::AuthRequired => Some(AgentTaskState::AuthRequired),
    }
}

fn slug(state: AgentTaskState) -> &'static str {
    match state {
        AgentTaskState::Submitted => "submitted",
        AgentTaskState::Working => "working",
        AgentTaskState::InputRequired => "input_required",
        AgentTaskState::AuthRequired => "auth_required",
        AgentTaskState::Completed => "completed",
        AgentTaskState::Failed => "failed",
        AgentTaskState::Canceled => "canceled",
        AgentTaskState::Rejected => "rejected",
    }
}

/// Text parts joined by newlines; `None` when there is no text.
fn text_of(parts: &[Part]) -> Option<String> {
    let texts: Vec<&str> = parts
        .iter()
        .filter_map(|p| match &p.content {
            PartContent::Text(t) => Some(t.as_str()),
            PartContent::Raw(_) | PartContent::Url(_) | PartContent::Data(_) => None,
        })
        .collect();
    let joined = texts.join("\n");
    (!joined.trim().is_empty()).then_some(joined)
}

/// Whether `part` says it carries A2UI: the A2A 1.0 `mediaType`, or the extension's
/// `metadata.mimeType`, is `application/a2ui+json` (parameters and case ignored).
fn claims_a2ui(part: &Part) -> bool {
    let is = |s: &str| {
        s.split(';')
            .next()
            .is_some_and(|m| m.trim().eq_ignore_ascii_case(A2UI_MEDIA_TYPE))
    };
    part.media_type.as_deref().is_some_and(is)
        || part
            .metadata
            .as_ref()
            .and_then(|m| m.get("mimeType"))
            .and_then(Value::as_str)
            .is_some_and(is)
}

/// The outcome of the envelope check for one A2UI part: its operations, or why it is refused.
type UiCheck = Result<Vec<Value>, String>;

/// A file part (`raw`): its position, and what it says of itself.
struct FilePart {
    index: usize,
    media_type: Option<String>,
    filename: Option<String>,
    bytes: Vec<u8>,
}

/// Parts split into what is A2UI (with each part's position), the files (`raw` parts, with their
/// positions) and what is neither.
struct Split {
    rest: Vec<Part>,
    ui: Vec<(usize, UiCheck)>,
    files: Vec<FilePart>,
}

/// `Some` only for text that is not empty.
fn nonempty(text: &Option<String>) -> Option<String> {
    text.as_ref().filter(|t| !t.trim().is_empty()).cloned()
}

/// Separates the A2UI parts, checking each one, and the files. A part that claims A2UI and is
/// not a data part is refused too: it is never treated as text.
fn split_ui(parts: &[Part]) -> Split {
    let mut split = Split {
        rest: Vec::new(),
        ui: Vec::new(),
        files: Vec::new(),
    };
    for (i, part) in parts.iter().enumerate() {
        if !claims_a2ui(part) {
            // A `raw` part is a file (ADR 0032): it is never text, and never dropped silently.
            if let PartContent::Raw(bytes) = &part.content {
                split.files.push(FilePart {
                    index: i,
                    media_type: nonempty(&part.media_type),
                    filename: nonempty(&part.filename),
                    bytes: bytes.clone(),
                });
            } else {
                split.rest.push(part.clone());
            }
            continue;
        }
        let check = match &part.content {
            PartContent::Data(v) => check_operations(v).map_err(|e| e.to_string()),
            PartContent::Text(_) | PartContent::Raw(_) | PartContent::Url(_) => {
                Err("an A2UI part must be a data part".to_owned())
            }
        };
        split.ui.push((i, check));
    }
    split
}

/// One envelope per A2UI part, in order. `key` names the part by its position.
fn ui_envelopes(
    task_id: &str,
    context_id: &str,
    revision: &Option<String>,
    ui: Vec<(usize, UiCheck)>,
    key: impl Fn(usize) -> IdemKey,
) -> Vec<AgentEnvelope> {
    ui.into_iter()
        .map(|(index, check)| AgentEnvelope {
            task_id: task_id.to_owned(),
            context_id: context_id.to_owned(),
            task_state: None,
            revision: revision.clone(),
            key: key(index),
            update: Some(match check {
                Ok(operations) => AgentUpdate::Ui { operations },
                Err(reason) => AgentUpdate::UiRejected { reason },
            }),
            live: None,
        })
        .collect()
}

/// The revision the agent echoes under the extension's key (`{requested, revision}`).
fn revision_of(metadata: &Metadata) -> Option<String> {
    metadata
        .as_ref()?
        .get(RELEASE_CHANNELS_URI)?
        .get("revision")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The `usage/v1` entry of a metadata map (ADR 0056).
fn usage_entry(metadata: &Metadata) -> Option<&Value> {
    metadata.as_ref()?.get(USAGE_EXTENSION)
}

/// The envelope of a call report on a `working` status update: the call, under a key that names
/// it (a resubscribe that replays the update, a poll and another replica collapse into one), or
/// what was wrong with it.
fn usage_call_envelope(
    task_id: &str,
    context_id: &str,
    entry: &Value,
    revision: Option<String>,
) -> AgentEnvelope {
    let (key, update) = match UsageCall::parse(task_id, entry) {
        Ok(call) => (
            IdemKey::Task(format!("a2a:{task_id}:usage:{}", call.call)),
            AgentUpdate::Usage(UsageUpdate::Call(call)),
        ),
        Err(why) => (
            IdemKey::Turn(format!("{task_id}:usage-rejected")),
            AgentUpdate::UsageRejected(why),
        ),
    };
    AgentEnvelope {
        task_id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        task_state: Some(AgentTaskState::Working),
        revision,
        key,
        update: Some(update),
        live: None,
    }
}

/// The envelope of a task's totals as it reached `state` (one that ends or pauses the turn), or of
/// what was wrong with them. It implies no state: the status that follows says it.
fn usage_totals_envelope(
    task_id: &str,
    context_id: &str,
    state: AgentTaskState,
    entry: &Value,
    revision: Option<String>,
) -> AgentEnvelope {
    let (key, update) = match UsageTotals::parse(task_id, entry) {
        Ok(totals) => (
            IdemKey::Turn(format!("{task_id}:usage-total:{}", slug(state))),
            AgentUpdate::Usage(UsageUpdate::Total(totals)),
        ),
        Err(why) => (
            IdemKey::Turn(format!("{task_id}:usage-rejected")),
            AgentUpdate::UsageRejected(why),
        ),
    };
    AgentEnvelope {
        task_id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        task_state: None,
        revision,
        key,
        update: Some(update),
        live: None,
    }
}

/// The task a stream item ends or pauses **without** its `usage/v1` totals: a status update whose
/// state ends the turn and whose metadata has no entry under the extension's URI (ADR 0056). A
/// streaming client never sees the task's own metadata, so the A2A adapter reads the task once
/// (`GetTask`) and passes [`usage_totals`] on before the status, for an agent whose card lists the
/// extension. `None` for any other item.
pub fn wants_usage_totals(item: &StreamResponse) -> Option<&str> {
    let StreamResponse::StatusUpdate(u) = item else {
        return None;
    };
    let ends = state_of(&u.status.state).is_some_and(AgentTaskState::ends_turn);
    (ends && usage_entry(&u.metadata).is_none() && !u.task_id.is_empty())
        .then_some(u.task_id.as_str())
}

/// The `usage/v1` totals a task holds in its own metadata, as the envelope a stream would have said
/// them in, when its state ends or pauses the turn (ADR 0056); `None` for a task that is still
/// working or holds none.
pub fn usage_totals(task: &Task) -> Option<AgentEnvelope> {
    let state = state_of(&task.status.state).filter(|s| s.ends_turn())?;
    let entry = usage_entry(&task.metadata)?;
    Some(usage_totals_envelope(
        &task.id,
        &task.context_id,
        state,
        entry,
        revision_of(&task.metadata),
    ))
}

/// The envelopes of a status: the A2UI parts of its message first, then the status itself.
fn status_envelopes(
    task_id: &str,
    context_id: &str,
    status: &TaskStatus,
    revision: Option<String>,
) -> Vec<AgentEnvelope> {
    let message = status.message.as_ref();
    let split = message.map(|m| split_ui(&m.parts)).unwrap_or(Split {
        rest: Vec::new(),
        ui: Vec::new(),
        files: Vec::new(),
    });
    let state = state_of(&status.state);
    let mut out = match message.filter(|m| !m.message_id.is_empty()) {
        Some(m) => ui_envelopes(task_id, context_id, &revision, split.ui, |i| {
            IdemKey::Task(format!("a2a:{task_id}:status-msg:{}:ui:{i}", m.message_id))
        }),
        None => ui_envelopes(task_id, context_id, &revision, split.ui, |i| {
            IdemKey::Turn(format!(
                "{task_id}:status-ui:{}:{i}",
                state.map_or("unspecified", slug)
            ))
        }),
    };
    // A step is a `working` status whose message says so; anything else is a plain status.
    let step = message
        .filter(|_| state == Some(AgentTaskState::Working))
        .and_then(|m| step_of(task_id, m));
    match step {
        Some(report) => out.push(step_envelope(
            task_id,
            context_id,
            message.map(|m| m.message_id.as_str()).unwrap_or_default(),
            report,
            revision,
        )),
        None => {
            // The whole text of a stream, stated once (`text-stream/v1`): it is a message of
            // the reply's own id, and the status that carries it says no more than that.
            let streamed = message.and_then(stream_of);
            if let Some((stream_id, text)) = &streamed {
                out.push(AgentEnvelope {
                    task_id: task_id.to_owned(),
                    context_id: context_id.to_owned(),
                    task_state: None,
                    revision: revision.clone(),
                    key: IdemKey::Task(format!("a2a:msg:{stream_id}")),
                    update: Some(AgentUpdate::Message {
                        message_id: stream_id.clone(),
                        text: text.clone(),
                        is_final: true,
                        purpose: purpose_of(state),
                    }),
                    live: None,
                });
            }
            let mut envelope = status_envelope(task_id, context_id, status, revision);
            if streamed.is_some()
                && state == Some(AgentTaskState::Working)
                && let Some(AgentUpdate::Status { detail, .. }) = &mut envelope.update
            {
                *detail = None;
            }
            out.push(envelope);
        }
    }
    out
}

/// What the words an agent stated on a status in `state` are for (ADR 0031): working text on a
/// `working` status, the turn's answer on a status that ends the turn, and nothing said on any
/// other (a failure's words are its error, not an answer).
fn purpose_of(state: Option<AgentTaskState>) -> Option<MessagePurpose> {
    match state? {
        AgentTaskState::Working => Some(MessagePurpose::Working),
        AgentTaskState::Completed
        | AgentTaskState::InputRequired
        | AgentTaskState::AuthRequired => Some(MessagePurpose::Answer),
        AgentTaskState::Submitted
        | AgentTaskState::Failed
        | AgentTaskState::Canceled
        | AgentTaskState::Rejected => None,
    }
}

/// The longest stream id an agent may send, in bytes (`docs/api/text-stream-v1.md`).
const MAX_STREAM_ID_BYTES: usize = 128;

/// A stream id: 1 to 128 bytes with no control character.
fn valid_stream_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_STREAM_ID_BYTES && !id.chars().any(char::is_control)
}

/// The stream a status message states the whole text of, and that text: a valid `streamId` under
/// the URI and a text that is not blank. `None` for any other message.
fn stream_of(message: &Message) -> Option<(String, String)> {
    let entry = message
        .metadata
        .as_ref()?
        .get(TEXT_STREAM_EXTENSION)?
        .as_object()?;
    let id = entry
        .get("streamId")?
        .as_str()
        .filter(|s| valid_stream_id(s))?;
    Some((id.to_owned(), text_of(&message.parts)?))
}

/// A JSON number that is a non-negative whole number. A2A's `metadata` is a protobuf `Struct`
/// (*verified 2026-10-01*: `10` arrives as `10.0` through the A2A SDK), so a whole number may be
/// written either way; one above 2^53 (not exactly representable) or with a fraction is not.
fn whole_number(value: &Value) -> Option<u64> {
    const MAX_EXACT: f64 = 9_007_199_254_740_991.0;
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    let f = value.as_f64()?;
    (f >= 0.0 && f.fract() == 0.0 && f <= MAX_EXACT).then_some(f as u64)
}

/// The entry an artifact carries under `text-stream/v1`, if it carries one.
fn text_stream_entry(artifact: &a2a::Artifact) -> Option<&serde_json::Map<String, Value>> {
    artifact
        .metadata
        .as_ref()?
        .get(TEXT_STREAM_EXTENSION)?
        .as_object()
}

/// What an artifact update is, under `text-stream/v1`.
enum Chunk {
    /// A chunk of a reply or of reasoning.
    Piece(LiveChunk),
    /// A chunk of a kind this reader does not know (`kind` is something but `"reasoning"`): it is
    /// neither shown nor made an artifact. A kind added later is ignored by a reader that predates it,
    /// as the contract says, and never read as a reply.
    Unknown,
}

/// The chunk an artifact update is, under `text-stream/v1`; `None` for an update that is not
/// one, or whose entry does not validate (it is then a plain artifact chunk).
fn live_chunk_of(update: &TaskArtifactUpdateEvent) -> Option<Chunk> {
    let artifact = &update.artifact;
    let entry = text_stream_entry(artifact)?;
    let offset = whole_number(entry.get("offset")?)?;
    if !valid_stream_id(&artifact.artifact_id) {
        return None;
    }
    let [part] = artifact.parts.as_slice() else {
        return None;
    };
    let PartContent::Text(text) = &part.content else {
        return None;
    };
    if claims_a2ui(part) {
        return None;
    }
    // What the stream is the words of: a reply when the entry says nothing, reasoning when it says so
    // (`docs/api/text-stream-v1.md` "Reasoning"). A kind this reader does not know is not a reply.
    let kind = match entry.get("kind") {
        None | Some(Value::Null) => LiveKind::Reply,
        Some(Value::String(kind)) if kind == "reasoning" => LiveKind::Reasoning,
        Some(_) => return Some(Chunk::Unknown),
    };
    // Giving up is said on the last chunk; said without `lastChunk` it ends the stream too.
    let abandoned = entry.get("abandoned").and_then(Value::as_bool) == Some(true);
    let end = if abandoned {
        LiveEnd::Abandoned
    } else if update.last_chunk == Some(true) {
        LiveEnd::Last
    } else {
        LiveEnd::Open
    };
    Some(Chunk::Piece(LiveChunk {
        message_id: artifact.artifact_id.clone(),
        offset,
        text: text.clone(),
        end,
        kind,
    }))
}

/// The envelope of a chunk: no update, only the piece, which is relayed and never applied.
fn live_envelope(update: &TaskArtifactUpdateEvent, chunk: LiveChunk) -> AgentEnvelope {
    AgentEnvelope {
        task_id: update.task_id.clone(),
        context_id: update.context_id.clone(),
        task_state: None,
        revision: revision_of(&update.metadata),
        key: IdemKey::Turn(format!(
            "{}:live:{}:{}",
            update.task_id, chunk.message_id, chunk.offset
        )),
        update: None,
        live: Some(chunk),
    }
}

/// The longest step id an agent may send, in bytes (`docs/api/steps-v1.md`); the task id and a
/// slash go in front of it, and the core cuts what is still too long for the log.
const MAX_AGENT_STEP_ID_BYTES: usize = 128;

/// An optional string member: absent or `null` is none, a string is itself, anything else is not
/// usable (the entry is then not a step).
fn optional_text<'a>(
    entry: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, ()> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(()),
    }
}

/// What a step reports it was called with: an object. Anything else (absent, `null`, a string, a
/// list) is no input, and the step stays: `input` and `output` are **lenient** on purpose
/// (ADR 0030), unlike the members that identify the step, so that an agent that sends them badly
/// loses only them.
fn input_of(entry: &serde_json::Map<String, Value>) -> Option<serde_json::Map<String, Value>> {
    entry.get("input")?.as_object().cloned()
}

/// What a step reports it returned: `{text, truncated?, bytes?, error?}`. Without a string
/// `text` there is no output; an optional member of the wrong type is read as absent.
fn output_of(entry: &serde_json::Map<String, Value>) -> Option<StepOutput> {
    let output = entry.get("output")?.as_object()?;
    Some(StepOutput {
        text: output.get("text")?.as_str()?.to_owned(),
        truncated: output
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        bytes: output.get("bytes").and_then(Value::as_u64),
        error: output
            .get("error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The step a status message reports under `steps/v1`, or `None` when it reports none: no entry
/// under the URI, or one that does not validate (no usable `id`, no `label`, a `state` that is not
/// one of the five, a member of the wrong type). The ids carry the task id, so they are unique
/// within the thread. `input` and `output` are read leniently ([`input_of`], [`output_of`]): a
/// bad one is dropped and the step is kept. What is left to the core's door
/// ([`StepReport::sanitize`]) is the cutting of long text, the icon vocabulary, the control
/// characters and the redaction.
fn step_of(task_id: &str, message: &Message) -> Option<StepReport> {
    let entry = message
        .metadata
        .as_ref()?
        .get(STEPS_EXTENSION)?
        .as_object()?;
    let id = optional_text(entry, "id").ok()??;
    if id.is_empty() || id.len() > MAX_AGENT_STEP_ID_BYTES || id.chars().any(char::is_control) {
        return None;
    }
    let label = optional_text(entry, "label").ok()??;
    if label.trim().is_empty() {
        return None;
    }
    let state = StepState::parse(optional_text(entry, "state").ok()??)?;
    let kind = match optional_text(entry, "kind").ok()? {
        Some(kind) => StepKind::parse(kind).unwrap_or(StepKind::Tool),
        None => StepKind::Tool,
    };
    let parent = optional_text(entry, "parentId")
        .ok()?
        .filter(|p| !p.is_empty())
        .map(|p| format!("{task_id}/{p}"));
    Some(StepReport {
        id: format!("{task_id}/{id}"),
        parent,
        kind,
        label: label.to_owned(),
        state,
        icon: optional_text(entry, "icon").ok()?.map(str::to_owned),
        detail: optional_text(entry, "detail").ok()?.map(str::to_owned),
        input: input_of(entry),
        output: output_of(entry),
    })
}

/// The envelope of a step: a `working` task with the step as its update. Its key names the step
/// and the status message it came in, so a replay, a resubscribe and a poll collapse into one.
fn step_envelope(
    task_id: &str,
    context_id: &str,
    message_id: &str,
    report: StepReport,
    revision: Option<String>,
) -> AgentEnvelope {
    // the key names the agent's own id: the task is in it already
    let own = report
        .id
        .strip_prefix(task_id)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(&report.id);
    let key = if message_id.is_empty() {
        IdemKey::Turn(format!("{task_id}:step:{own}:{}", report.state.as_str()))
    } else {
        IdemKey::Task(format!("a2a:{task_id}:step:{own}:{message_id}"))
    };
    AgentEnvelope {
        task_id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        task_state: Some(AgentTaskState::Working),
        revision,
        key,
        update: Some(AgentUpdate::Step(report)),
        live: None,
    }
}

fn status_envelope(
    task_id: &str,
    context_id: &str,
    status: &TaskStatus,
    revision: Option<String>,
) -> AgentEnvelope {
    let state = state_of(&status.state);
    let (key, update) = match state {
        None | Some(AgentTaskState::Submitted) => {
            (IdemKey::Turn(format!("{task_id}:status:submitted")), None)
        }
        Some(state) => {
            let detail = status.message.as_ref().and_then(|m| text_of(&m.parts));
            let key = match status.message.as_ref().filter(|m| !m.message_id.is_empty()) {
                Some(m) => IdemKey::Task(format!("a2a:{task_id}:status-msg:{}", m.message_id)),
                None => IdemKey::Turn(format!("{task_id}:status:{}", slug(state))),
            };
            (key, Some(AgentUpdate::Status { state, detail }))
        }
    };
    AgentEnvelope {
        task_id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        task_state: state,
        revision,
        key,
        update,
        live: None,
    }
}

fn artifact_update(artifact_id: &str, name: Option<&str>, parts: &[Part]) -> AgentUpdate {
    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or(artifact_id)
        .to_owned();
    let uri = parts.iter().find_map(|p| match &p.content {
        PartContent::Url(u) => Some(u.clone()),
        PartContent::Text(_) | PartContent::Raw(_) | PartContent::Data(_) => None,
    });
    let mut pieces: Vec<String> = Vec::new();
    let mut only_data = true;
    for p in parts {
        match &p.content {
            PartContent::Text(t) => {
                only_data = false;
                pieces.push(t.clone());
            }
            PartContent::Data(v) => pieces.push(v.to_string()),
            PartContent::Url(_) | PartContent::Raw(_) => only_data = false,
        }
    }
    let mime_type = parts
        .iter()
        .find_map(|p| p.media_type.clone())
        .or_else(|| (only_data && !pieces.is_empty()).then(|| "application/json".to_owned()));
    let joined = pieces.join("\n");
    AgentUpdate::Artifact {
        name,
        mime_type,
        uri,
        text: (!joined.is_empty()).then_some(joined),
    }
}

/// The text of an artifact that is nothing but text: its parts, every one of them text, joined by
/// newlines. `None` for an artifact with no part, or with a part that is data, a file or a link.
fn plain_text(parts: &[Part]) -> Option<String> {
    if parts.is_empty() {
        return None;
    }
    let mut texts: Vec<&str> = Vec::with_capacity(parts.len());
    for p in parts {
        match &p.content {
            PartContent::Text(t) => texts.push(t.as_str()),
            PartContent::Raw(_) | PartContent::Url(_) | PartContent::Data(_) => return None,
        }
    }
    Some(texts.join("\n"))
}

/// The text of an artifact that may be the turn's answer (ADR 0031, amendment of 2026-10-07): it
/// names nothing (an agent that names an artifact says it is a deliverable, not its words) and
/// is nothing but text.
fn answer_text(name: Option<&str>, parts: &[Part]) -> Option<String> {
    if name.is_some_and(|n| !n.trim().is_empty()) {
        return None;
    }
    plain_text(parts)
}

/// The one message that is the answer of a turn that gave it as unnamed text artifacts: the
/// artifacts' texts, a blank line between them, under a message id that names the task and the first
/// artifact (so a turn continued on the same task, after a question, has a second answer of its own).
fn answer_envelope(
    task_id: &str,
    context_id: &str,
    first_artifact: &str,
    text: String,
    revision: Option<String>,
) -> AgentEnvelope {
    let message_id = format!("{task_id}:artifact:{first_artifact}");
    AgentEnvelope {
        task_id: task_id.to_owned(),
        context_id: context_id.to_owned(),
        task_state: None,
        revision,
        key: IdemKey::Task(format!("a2a:msg:{message_id}")),
        update: Some(AgentUpdate::Message {
            message_id,
            text,
            is_final: true,
            purpose: Some(MessagePurpose::Answer),
        }),
        live: None,
    }
}

/// The envelopes of a whole artifact: the artifact of its ordinary parts (none when every part
/// is A2UI), then one envelope per A2UI part.
fn artifact_envelopes(
    task_id: &str,
    context_id: &str,
    artifact_id: &str,
    name: Option<&str>,
    parts: &[Part],
    revision: Option<String>,
) -> Vec<AgentEnvelope> {
    let split = split_ui(parts);
    let mut out = Vec::new();
    // The artifact itself, unless every part is a file or A2UI.
    if !split.rest.is_empty() || (split.ui.is_empty() && split.files.is_empty()) {
        out.push(AgentEnvelope {
            task_id: task_id.to_owned(),
            context_id: context_id.to_owned(),
            task_state: None,
            revision: revision.clone(),
            key: IdemKey::Task(format!("a2a:{task_id}:artifact:{artifact_id}")),
            update: Some(artifact_update(artifact_id, name, &split.rest)),
            live: None,
        });
    }
    // One file per `raw` part, in order, each under its own key (the part's position).
    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or(artifact_id)
        .to_owned();
    for file in split.files {
        out.push(AgentEnvelope {
            task_id: task_id.to_owned(),
            context_id: context_id.to_owned(),
            task_state: None,
            revision: revision.clone(),
            key: IdemKey::Task(format!(
                "a2a:{task_id}:artifact:{artifact_id}:file:{}",
                file.index
            )),
            update: Some(AgentUpdate::File {
                name: name.clone(),
                media_type: file.media_type,
                filename: file.filename,
                bytes: file.bytes,
            }),
            live: None,
        });
    }
    out.extend(ui_envelopes(
        task_id,
        context_id,
        &revision,
        split.ui,
        |i| IdemKey::Task(format!("a2a:{task_id}:artifact:{artifact_id}:ui:{i}")),
    ));
    out
}

/// An artifact of a task as a poll holds it: the entries that share its id, merged.
struct Merged<'a> {
    name: Option<&'a str>,
    parts: Vec<Part>,
    /// Its text when it may be the answer: the entries' texts, concatenated as the stream does.
    answer: Option<String>,
}

/// Every artifact of a task, merging entries that share an id (the server appends chunk by
/// chunk without merging), then the status. A task that is `completed` with no words of its own
/// and holds unnamed text artifacts says them as its answer instead of as artifacts (ADR 0031,
/// amendment of 2026-10-07): a poll cannot know what the stream said before it, and an agent
/// that stated its answer says it on the status, so the rule is the stream's, without the said
/// message.
fn task_envelopes(task: &Task) -> Vec<AgentEnvelope> {
    let revision = revision_of(&task.metadata);
    let mut order: Vec<&str> = Vec::new();
    let mut merged: HashMap<&str, Merged<'_>> = HashMap::new();
    // A chunk of streamed text is transient: a task that holds one (an agent that kept it) is not
    // showing an artifact.
    for a in task
        .artifacts
        .iter()
        .flatten()
        .filter(|a| text_stream_entry(a).is_none())
    {
        match merged.get_mut(a.artifact_id.as_str()) {
            Some(m) => {
                m.answer = m
                    .answer
                    .take()
                    .and_then(|t| plain_text(&a.parts).map(|more| t + &more));
                m.parts.extend(a.parts.iter().cloned());
            }
            None => {
                order.push(a.artifact_id.as_str());
                merged.insert(
                    a.artifact_id.as_str(),
                    Merged {
                        name: a.name.as_deref(),
                        parts: a.parts.clone(),
                        answer: answer_text(a.name.as_deref(), &a.parts),
                    },
                );
            }
        }
    }
    let words = task
        .status
        .message
        .as_ref()
        .and_then(|m| text_of(&m.parts))
        .is_some();
    let is_answer = |m: &Merged<'_>| {
        task.status.state == TaskState::Completed
            && !words
            && m.answer.as_deref().is_some_and(|t| !t.trim().is_empty())
    };
    let mut answer: Option<(&str, String)> = None;
    let mut out: Vec<AgentEnvelope> = Vec::new();
    for id in order {
        let Some(m) = merged.get(id) else { continue };
        if is_answer(m) {
            let text = m.answer.clone().unwrap_or_default();
            answer = Some(match answer.take() {
                Some((first, said)) => (first, format!("{said}\n\n{text}")),
                None => (id, text),
            });
            continue;
        }
        out.extend(artifact_envelopes(
            &task.id,
            &task.context_id,
            id,
            m.name,
            &m.parts,
            revision.clone(),
        ));
    }
    if let Some((first, text)) = answer {
        out.push(answer_envelope(
            &task.id,
            &task.context_id,
            first,
            text,
            revision.clone(),
        ));
    }
    // the task's totals, before the status that ends or pauses it (ADR 0056)
    out.extend(usage_totals(task));
    out.extend(status_envelopes(
        &task.id,
        &task.context_id,
        &task.status,
        revision,
    ));
    out
}

/// A polled view of a task. An unspecified state is a protocol violation.
pub fn snapshot(task: &Task) -> Result<TaskSnapshot, AgentError> {
    let state = state_of(&task.status.state).ok_or_else(|| {
        AgentError::protocol(format!("task {} reports an unspecified state", task.id))
    })?;
    Ok(TaskSnapshot {
        task_id: task.id.clone(),
        context_id: task.context_id.clone(),
        state,
        revision: revision_of(&task.metadata),
        envelopes: task_envelopes(task),
    })
}

/// An artifact held back because more chunks may follow.
struct Pending {
    task_id: String,
    context_id: String,
    artifact_id: String,
    name: Option<String>,
    parts: Vec<Part>,
    revision: Option<String>,
    /// Its text when it may be the turn's answer (`answer_text`): the chunks as written.
    answer: Option<String>,
}

impl Pending {
    fn into_envelopes(self) -> Vec<AgentEnvelope> {
        artifact_envelopes(
            &self.task_id,
            &self.context_id,
            &self.artifact_id,
            self.name.as_deref(),
            &self.parts,
            self.revision,
        )
    }
}

/// What the unnamed text artifacts a turn kept say when the task completes with nothing said:
/// one answer message of their texts, in order (ADR 0031, amendment of 2026-10-07). `None` when
/// none is kept.
fn held_answer(
    task_id: &str,
    context_id: &str,
    held: Vec<Pending>,
    revision: Option<String>,
) -> Option<Vec<AgentEnvelope>> {
    let first = held.first()?.artifact_id.clone();
    let text = held
        .into_iter()
        .filter_map(|p| p.answer)
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(vec![answer_envelope(
        task_id, context_id, &first, text, revision,
    )])
}

/// The most reasoning streams followed at once by one mapper; a stream beyond that is relayed and not logged.
const MAX_OPEN_REASONING: usize = 8;

/// The reasoning of one stream, collected from its chunks while they pass (ADR 0044).
struct Collected {
    id: String,
    text: String,
    /// Where the next chunk is expected to begin, in UTF-8 bytes.
    next: u64,
    /// The text is not the whole reasoning: it was cut at the bound or a piece was lost.
    truncated: bool,
}

impl Collected {
    /// Takes the part of the chunk that continues the text; an overlap is trimmed, and a gap or a
    /// chunk past the bound makes the text a truncated one (nothing more is added to it).
    fn add(&mut self, offset: u64, piece: &str) {
        let (offset, piece) = if offset < self.next {
            let skip = usize::try_from(self.next - offset).unwrap_or(usize::MAX);
            match piece.get(skip..) {
                Some(rest) => (self.next, rest),
                None if skip >= piece.len() => (self.next, ""),
                // The cut is not on a character: the pieces do not agree with each other.
                None => {
                    self.truncated = true;
                    return;
                }
            }
        } else {
            (offset, piece)
        };
        if offset > self.next {
            // A piece was lost: what is held is the beginning, and it is said to be so.
            self.truncated = true;
            return;
        }
        self.next = offset + piece.len() as u64;
        if self.truncated {
            return;
        }
        let room = MAX_REASONING_BYTES.saturating_sub(self.text.len());
        if piece.len() <= room {
            self.text.push_str(piece);
        } else {
            let mut cut = room;
            while !piece.is_char_boundary(cut) {
                cut -= 1;
            }
            self.text.push_str(&piece[..cut]);
            self.truncated = true;
        }
    }
}

/// Stream-local state: the task the stream belongs to, the artifact held back (if any), the
/// reasoning being collected, and what the stream has said so far (ADR 0031, amendment of
/// 2026-10-07).
#[derive(Default)]
pub struct StreamMapper {
    task_id: Option<String>,
    context_id: Option<String>,
    pending: Option<Pending>,
    reasoning: Vec<Collected>,
    /// Finished unnamed text artifacts, kept until the turn ends: they are the answer when the
    /// task completes with nothing said, and artifacts otherwise.
    held: Vec<Pending>,
    /// The stream has stated a message (a streamed text, a plain A2A `Message`): the agent says
    /// its words that way, so its text artifacts are artifacts.
    said: bool,
}

impl StreamMapper {
    fn learn(&mut self, task_id: &str, context_id: &str) {
        if !task_id.is_empty() {
            self.task_id = Some(task_id.to_owned());
        }
        if !context_id.is_empty() {
            self.context_id = Some(context_id.to_owned());
        }
    }

    /// Maps one stream item to zero or more envelopes (or an error).
    pub fn map(&mut self, item: StreamResponse) -> Vec<Result<AgentEnvelope, AgentError>> {
        if let StreamResponse::ArtifactUpdate(u) = item {
            self.learn(&u.task_id, &u.context_id);
            if let Some(read) = live_chunk_of(&u) {
                // Not an artifact, and an event like any other for one that was held back.
                let mut out: Vec<Result<AgentEnvelope, AgentError>> =
                    self.finish_pending().into_iter().map(Ok).collect();
                if let Chunk::Piece(chunk) = read {
                    let logged = self.collect_reasoning(&u, &chunk);
                    out.push(Ok(live_envelope(&u, chunk)));
                    out.extend(logged.into_iter().map(Ok));
                }
                return out;
            }
            return self.artifact_update(u).into_iter().map(Ok).collect();
        }
        // Any other event ends a held-back artifact: nothing more is appended to it.
        let mut out: Vec<Result<AgentEnvelope, AgentError>> =
            self.finish_pending().into_iter().map(Ok).collect();
        match item {
            StreamResponse::Task(task) => {
                self.learn(&task.id, &task.context_id);
                out.extend(self.release_held().into_iter().map(Ok));
                out.extend(task_envelopes(&task).into_iter().map(Ok));
            }
            StreamResponse::StatusUpdate(u) => {
                self.learn(&u.task_id, &u.context_id);
                let state = state_of(&u.status.state);
                let revision = revision_of(&u.metadata);
                match state {
                    // The turn goes on: what is held waits for its end.
                    None | Some(AgentTaskState::Submitted | AgentTaskState::Working) => {}
                    Some(AgentTaskState::Completed) => {
                        let words = u
                            .status
                            .message
                            .as_ref()
                            .and_then(|m| text_of(&m.parts))
                            .is_some();
                        let held = std::mem::take(&mut self.held);
                        if self.said || words {
                            out.extend(held.into_iter().flat_map(Pending::into_envelopes).map(Ok));
                        } else if let Some(answer) =
                            held_answer(&u.task_id, &u.context_id, held, revision.clone())
                        {
                            out.extend(answer.into_iter().map(Ok));
                        }
                    }
                    // It ends or waits without completing: what it handed over is artifacts.
                    Some(
                        AgentTaskState::Failed
                        | AgentTaskState::Canceled
                        | AgentTaskState::InputRequired
                        | AgentTaskState::Rejected
                        | AgentTaskState::AuthRequired,
                    ) => out.extend(self.release_held().into_iter().map(Ok)),
                }
                // usage/v1 (ADR 0056): a call report on a `working` update, which without a message
                // is all the update says; the task's totals before a status that ends or pauses it
                let report_only = match (state, usage_entry(&u.metadata)) {
                    (Some(AgentTaskState::Working), Some(entry)) => {
                        out.push(Ok(usage_call_envelope(
                            &u.task_id,
                            &u.context_id,
                            entry,
                            revision.clone(),
                        )));
                        u.status.message.is_none()
                    }
                    (Some(state), Some(entry)) if state.ends_turn() => {
                        out.push(Ok(usage_totals_envelope(
                            &u.task_id,
                            &u.context_id,
                            state,
                            entry,
                            revision.clone(),
                        )));
                        false
                    }
                    _ => false,
                };
                if !report_only {
                    out.extend(
                        status_envelopes(&u.task_id, &u.context_id, &u.status, revision)
                            .into_iter()
                            .map(Ok),
                    );
                }
            }
            StreamResponse::Message(m) => {
                out.extend(self.release_held().into_iter().map(Ok));
                out.extend(self.message(&m));
            }
            StreamResponse::ArtifactUpdate(_) => {}
        }
        self.said |= out.iter().any(|e| {
            matches!(
                e,
                Ok(AgentEnvelope {
                    update: Some(AgentUpdate::Message { .. }),
                    ..
                })
            )
        });
        out
    }

    /// The artifact held back, now that nothing more is appended to it: kept for the end of the
    /// turn when it may be the answer, an artifact otherwise.
    fn finish_pending(&mut self) -> Vec<AgentEnvelope> {
        match self.pending.take() {
            Some(p) => self.keep_or_say(p),
            None => Vec::new(),
        }
    }

    fn keep_or_say(&mut self, p: Pending) -> Vec<AgentEnvelope> {
        if p.answer.as_deref().is_some_and(|t| !t.trim().is_empty()) {
            self.held.push(p);
            Vec::new()
        } else {
            p.into_envelopes()
        }
    }

    /// The unnamed text artifacts kept for the end of the turn, as the artifacts they are.
    fn release_held(&mut self) -> Vec<AgentEnvelope> {
        std::mem::take(&mut self.held)
            .into_iter()
            .flat_map(Pending::into_envelopes)
            .collect()
    }

    /// Collects the chunks of a reasoning stream and, when the stream ends, the envelope that logs it
    /// whole (ADR 0044): `AgentUpdate::Reasoning`, under the key `a2a:<task>:reasoning:<stream>`, so a
    /// repeat collapses. A stream whose beginning this mapper did not see (a resubscribe joined it
    /// mid-way) is not logged, and nor is one that says nothing but blanks; one the agent gave up is
    /// logged as far as it went, marked truncated.
    fn collect_reasoning(
        &mut self,
        u: &TaskArtifactUpdateEvent,
        chunk: &LiveChunk,
    ) -> Option<AgentEnvelope> {
        if chunk.kind != LiveKind::Reasoning {
            return None;
        }
        let at = self.reasoning.iter().position(|c| c.id == chunk.message_id);
        let at = match at {
            Some(at) => at,
            None if chunk.offset == 0 => {
                if self.reasoning.len() == MAX_OPEN_REASONING {
                    self.reasoning.remove(0);
                }
                self.reasoning.push(Collected {
                    id: chunk.message_id.clone(),
                    text: String::new(),
                    next: 0,
                    truncated: false,
                });
                self.reasoning.len() - 1
            }
            // Joined mid-way: the beginning is not held, so the whole is not either.
            None => return None,
        };
        self.reasoning[at].add(chunk.offset, &chunk.text);
        if chunk.end == LiveEnd::Open {
            return None;
        }
        let done = self.reasoning.remove(at);
        if done.text.trim().is_empty() {
            return None;
        }
        Some(AgentEnvelope {
            task_id: u.task_id.clone(),
            context_id: u.context_id.clone(),
            task_state: None,
            revision: revision_of(&u.metadata),
            key: IdemKey::Task(format!("a2a:{}:reasoning:{}", u.task_id, done.id)),
            update: Some(AgentUpdate::Reasoning {
                message_id: done.id,
                text: done.text,
                truncated: done.truncated || chunk.end == LiveEnd::Abandoned,
            }),
            live: None,
        })
    }

    fn artifact_update(&mut self, u: TaskArtifactUpdateEvent) -> Vec<AgentEnvelope> {
        let revision = revision_of(&u.metadata);
        let append = u.append.unwrap_or(false);
        let last = u.last_chunk == Some(true);
        let same_artifact = self
            .pending
            .as_ref()
            .is_some_and(|p| p.artifact_id == u.artifact.artifact_id);

        let mut out: Vec<AgentEnvelope> = Vec::new();
        let held = if same_artifact && append {
            let mut p = self.pending.take();
            if let Some(p) = p.as_mut() {
                // the chunks of a text are one text, as written
                p.answer = p
                    .answer
                    .take()
                    .and_then(|t| plain_text(&u.artifact.parts).map(|more| t + &more));
                p.parts.extend(u.artifact.parts);
                p.revision = revision.or(p.revision.take());
            }
            p
        } else {
            // A different artifact, or the same id sent whole again (which replaces it).
            if !same_artifact {
                out.extend(self.finish_pending());
            } else {
                self.pending = None;
            }
            Some(Pending {
                task_id: u.task_id,
                context_id: u.context_id,
                artifact_id: u.artifact.artifact_id,
                answer: answer_text(u.artifact.name.as_deref(), &u.artifact.parts),
                name: u.artifact.name,
                parts: u.artifact.parts,
                revision,
            })
        };
        match held {
            Some(p) if last => out.extend(self.keep_or_say(p)),
            other => self.pending = other,
        }
        out
    }

    fn message(&mut self, m: &Message) -> Vec<Result<AgentEnvelope, AgentError>> {
        match m.role {
            Role::Agent => {}
            Role::User | Role::Unspecified => return Vec::new(),
        }
        let text = text_of(&m.parts);
        let split = split_ui(&m.parts);
        if text.is_none() && split.ui.is_empty() {
            return Vec::new();
        }
        let task_id = m.task_id.clone().filter(|t| !t.is_empty());
        let Some(task_id) = task_id.or_else(|| self.task_id.clone()) else {
            return vec![Err(AgentError::Unsupported(
                "the agent answered with a message but no task; only task-based agents are supported"
                    .to_owned(),
            ))];
        };
        let context_id = m
            .context_id
            .clone()
            .filter(|c| !c.is_empty())
            .or_else(|| self.context_id.clone())
            .unwrap_or_default();
        self.learn(&task_id, &context_id);
        let revision = revision_of(&m.metadata);
        let mut out: Vec<Result<AgentEnvelope, AgentError>> = Vec::new();
        if let Some(text) = text {
            out.push(Ok(AgentEnvelope {
                task_id: task_id.clone(),
                context_id: context_id.clone(),
                task_state: None,
                revision: revision.clone(),
                key: IdemKey::Task(format!("a2a:msg:{}", m.message_id)),
                update: Some(AgentUpdate::Message {
                    message_id: m.message_id.clone(),
                    text,
                    is_final: true,
                    // a plain A2A `Message` says nothing about what its words are for
                    purpose: None,
                }),
                live: None,
            }));
        }
        out.extend(
            ui_envelopes(&task_id, &context_id, &revision, split.ui, |i| {
                IdemKey::Task(format!("a2a:msg:{}:ui:{i}", m.message_id))
            })
            .into_iter()
            .map(Ok),
        );
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use a2a::{Artifact, TaskStatusUpdateEvent};
    use serde_json::json;

    use super::*;

    const T: &str = "task-1";
    const C: &str = "ctx-1";

    fn msg(id: &str, role: Role, text: &str) -> Message {
        let mut m = Message::new(role, vec![Part::text(text)]);
        m.message_id = id.to_owned();
        m
    }

    fn status(state: TaskState, message: Option<Message>) -> TaskStatus {
        TaskStatus {
            state,
            message,
            timestamp: None,
        }
    }

    fn status_update(state: TaskState, message: Option<Message>) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: T.into(),
            context_id: C.into(),
            status: status(state, message),
            metadata: None,
        })
    }

    fn art(id: &str, name: Option<&str>, parts: Vec<Part>) -> Artifact {
        Artifact {
            artifact_id: id.into(),
            name: name.map(str::to_owned),
            description: None,
            parts,
            metadata: None,
            extensions: None,
        }
    }

    fn artifact_update(a: Artifact, append: Option<bool>, last: Option<bool>) -> StreamResponse {
        StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: T.into(),
            context_id: C.into(),
            artifact: a,
            append,
            last_chunk: last,
            metadata: None,
        })
    }

    fn task(state: TaskState, artifacts: Vec<Artifact>, message: Option<Message>) -> Task {
        Task {
            id: T.into(),
            context_id: C.into(),
            status: status(state, message),
            artifacts: Some(artifacts),
            history: Some(vec![msg("h1", Role::Agent, "history is not replayed")]),
            metadata: None,
        }
    }

    fn only(mut v: Vec<Result<AgentEnvelope, AgentError>>) -> AgentEnvelope {
        assert_eq!(v.len(), 1, "{v:?}");
        v.remove(0).unwrap()
    }

    #[test]
    fn status_without_message_uses_a_turn_scoped_key() {
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, None)));
        assert_eq!(env.task_id, T);
        assert_eq!(env.context_id, C);
        assert_eq!(env.task_state, Some(AgentTaskState::Working));
        assert_eq!(env.key, IdemKey::Turn("task-1:status:working".into()));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Status {
                state: AgentTaskState::Working,
                detail: None
            })
        );
    }

    #[test]
    fn status_with_message_uses_a_task_scoped_key_and_carries_the_text() {
        let m = msg("m-7", Role::Agent, "Which branch?");
        let env =
            only(StreamMapper::default().map(status_update(TaskState::InputRequired, Some(m))));
        assert_eq!(env.key, IdemKey::Task("a2a:task-1:status-msg:m-7".into()));
        assert_eq!(env.task_state, Some(AgentTaskState::InputRequired));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Status {
                state: AgentTaskState::InputRequired,
                detail: Some("Which branch?".into())
            })
        );
    }

    #[test]
    fn every_state_maps() {
        let expect = [
            (TaskState::Submitted, AgentTaskState::Submitted),
            (TaskState::Working, AgentTaskState::Working),
            (TaskState::Completed, AgentTaskState::Completed),
            (TaskState::Failed, AgentTaskState::Failed),
            (TaskState::Canceled, AgentTaskState::Canceled),
            (TaskState::InputRequired, AgentTaskState::InputRequired),
            (TaskState::Rejected, AgentTaskState::Rejected),
            (TaskState::AuthRequired, AgentTaskState::AuthRequired),
        ];
        for (a2a_state, want) in expect {
            let env = only(StreamMapper::default().map(status_update(a2a_state, None)));
            assert_eq!(env.task_state, Some(want));
        }
        assert_eq!(state_of(&TaskState::Unspecified), None);
    }

    #[test]
    fn submitted_and_unspecified_only_carry_the_task_id() {
        let env = only(StreamMapper::default().map(status_update(TaskState::Submitted, None)));
        assert_eq!(env.update, None);
        assert_eq!(env.task_id, T);
        assert_eq!(env.task_state, Some(AgentTaskState::Submitted));
        let env = only(StreamMapper::default().map(status_update(TaskState::Unspecified, None)));
        assert_eq!(env.update, None);
        assert_eq!(env.task_state, None);
    }

    #[test]
    fn whole_artifact_maps_name_uri_text_and_key() {
        let a = art(
            "a-1",
            Some("result"),
            vec![
                Part::text("done"),
                Part::url("https://github.com/acme/demo/pull/1"),
            ],
        );
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert_eq!(env.key, IdemKey::Task("a2a:task-1:artifact:a-1".into()));
        assert_eq!(env.task_state, None);
        assert_eq!(
            env.update,
            Some(AgentUpdate::Artifact {
                name: "result".into(),
                mime_type: None,
                uri: Some("https://github.com/acme/demo/pull/1".into()),
                text: Some("done".into()),
            })
        );
    }

    #[test]
    fn an_artifact_not_marked_last_is_held_until_the_next_event() {
        let mut m = StreamMapper::default();
        let a = art("a-1", Some("result"), vec![Part::text("x")]);
        assert!(m.map(artifact_update(a, None, None)).is_empty());
        let out = m.map(status_update(TaskState::Completed, None));
        assert_eq!(out.len(), 2, "the artifact comes out before the status");
        let envs: Vec<_> = out.into_iter().map(Result::unwrap).collect();
        assert!(matches!(envs[0].update, Some(AgentUpdate::Artifact { .. })));
        assert_eq!(envs[1].task_state, Some(AgentTaskState::Completed));
    }

    #[test]
    fn a_stream_that_ends_drops_a_held_back_artifact() {
        // The dispatcher then polls GetTask, whose snapshot has the artifact whole, same key.
        let mut m = StreamMapper::default();
        let a = art("a-1", None, vec![Part::text("partial")]);
        assert!(m.map(artifact_update(a, None, None)).is_empty());
        assert!(m.pending.is_some());
    }

    #[test]
    fn artifact_without_name_falls_back_to_its_id_and_data_becomes_json_text() {
        let a = art("a-2", None, vec![Part::data(json!({"ok": true}))]);
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Artifact {
                name: "a-2".into(),
                mime_type: Some("application/json".into()),
                uri: None,
                text: Some(r#"{"ok":true}"#.into()),
            })
        );
    }

    /// ADR 0032: a `raw` part is a file, with what the agent said of it, under its own key.
    #[test]
    fn a_raw_part_is_a_file_and_nothing_else() {
        let a = art(
            "a-file",
            Some("chart"),
            vec![
                Part::raw(vec![1, 2, 3])
                    .with_media_type("image/png")
                    .with_filename("chart.png"),
            ],
        );
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert_eq!(
            env.key,
            IdemKey::Task("a2a:task-1:artifact:a-file:file:0".into())
        );
        assert_eq!(
            env.update,
            Some(AgentUpdate::File {
                name: "chart".into(),
                media_type: Some("image/png".into()),
                filename: Some("chart.png".into()),
                bytes: vec![1, 2, 3],
            })
        );
    }

    #[test]
    fn a_file_without_a_name_or_types_falls_back_and_blank_text_is_none() {
        let mut part = Part::raw(vec![9]).with_filename("  ");
        part.media_type = Some(String::new());
        let env = only(StreamMapper::default().map(artifact_update(
            art("a-bare", None, vec![part]),
            None,
            Some(true),
        )));
        assert_eq!(
            env.update,
            Some(AgentUpdate::File {
                name: "a-bare".into(),
                media_type: None,
                filename: None,
                bytes: vec![9],
            })
        );
    }

    /// An artifact of text and two files: the text is the artifact, each file is its own update
    /// under the key of its position, so a replay and a poll collapse into the same keys.
    #[test]
    fn text_and_files_in_one_artifact_are_one_artifact_and_one_update_per_file() {
        let parts = vec![
            Part::text("see the files"),
            Part::raw(vec![1]).with_filename("a.txt"),
            Part::raw(vec![2]).with_filename("b.txt"),
        ];
        let live = StreamMapper::default()
            .map(artifact_update(
                art("a-mix", Some("report"), parts.clone()),
                None,
                Some(true),
            ))
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        let polled = snapshot(&task(
            TaskState::Completed,
            vec![art("a-mix", Some("report"), parts)],
            None,
        ))
        .unwrap()
        .envelopes;
        let keys = |v: &[AgentEnvelope]| v.iter().map(|e| e.key.clone()).collect::<Vec<_>>();
        assert_eq!(
            keys(&live),
            [
                IdemKey::Task("a2a:task-1:artifact:a-mix".into()),
                IdemKey::Task("a2a:task-1:artifact:a-mix:file:1".into()),
                IdemKey::Task("a2a:task-1:artifact:a-mix:file:2".into()),
            ]
        );
        assert_eq!(keys(&live), keys(&polled[..3]));
        assert_eq!(
            live[0].update,
            Some(AgentUpdate::Artifact {
                name: "report".into(),
                mime_type: None,
                uri: None,
                text: Some("see the files".into()),
            })
        );
    }

    /// A `url` part stays a link here: this crate does no I/O, so fetching one from an allowed
    /// host is the adapter's.
    #[test]
    fn a_url_part_stays_a_link() {
        let a = art(
            "a-url",
            Some("report"),
            vec![Part::url("https://files.example.com/r.pdf").with_media_type("application/pdf")],
        );
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Artifact {
                name: "report".into(),
                mime_type: Some("application/pdf".into()),
                uri: Some("https://files.example.com/r.pdf".into()),
                text: None,
            })
        );
    }

    /// A raw part that claims to be A2UI is refused as A2UI, never taken for a file.
    #[test]
    fn a_raw_part_that_claims_a2ui_is_refused_not_kept() {
        let a = art(
            "a-ui",
            Some("ui"),
            vec![Part::raw(vec![1]).with_media_type(A2UI_MEDIA_TYPE)],
        );
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert!(matches!(env.update, Some(AgentUpdate::UiRejected { .. })));
    }

    #[test]
    fn media_type_of_the_first_part_wins() {
        let a = art(
            "a-3",
            Some("patch"),
            vec![Part::text("diff").with_media_type("text/x-diff")],
        );
        let env = only(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        let Some(AgentUpdate::Artifact { mime_type, .. }) = env.update else {
            panic!("not an artifact");
        };
        assert_eq!(mime_type.as_deref(), Some("text/x-diff"));
    }

    #[test]
    fn appended_chunks_are_emitted_once_on_the_last_chunk() {
        let mut m = StreamMapper::default();
        // On the wire the first chunk's `append: false, lastChunk: false` arrive as unset.
        let first = art("a-4", Some("log"), vec![Part::text("one ")]);
        assert!(m.map(artifact_update(first, None, None)).is_empty());
        let mid = art("a-4", None, vec![Part::text("two ")]);
        assert!(m.map(artifact_update(mid, Some(true), None)).is_empty());
        let last = art("a-4", None, vec![Part::text("three")]);
        let env = only(m.map(artifact_update(last, Some(true), Some(true))));
        assert_eq!(env.key, IdemKey::Task("a2a:task-1:artifact:a-4".into()));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Artifact {
                name: "log".into(),
                mime_type: None,
                uri: None,
                text: Some("one \ntwo \nthree".into()),
            })
        );
        assert!(m.pending.is_none(), "nothing is left held back");
    }

    #[test]
    fn another_artifact_ends_the_held_back_one() {
        let mut m = StreamMapper::default();
        let a = art("a-8", Some("first"), vec![Part::text("1")]);
        assert!(m.map(artifact_update(a, None, None)).is_empty());
        let b = art("a-9", Some("second"), vec![Part::text("2")]);
        let out = m.map(artifact_update(b, None, Some(true)));
        let names: Vec<_> = out
            .into_iter()
            .map(|e| match e.unwrap().update {
                Some(AgentUpdate::Artifact { name, .. }) => name,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["first", "second"]);
    }

    #[test]
    fn the_same_artifact_sent_whole_again_replaces_the_held_back_one() {
        let mut m = StreamMapper::default();
        let v1 = art("a-8", Some("n"), vec![Part::text("old")]);
        assert!(m.map(artifact_update(v1, None, None)).is_empty());
        let v2 = art("a-8", Some("n"), vec![Part::text("new")]);
        let env = only(m.map(artifact_update(v2, None, Some(true))));
        let Some(AgentUpdate::Artifact { text, .. }) = env.update else {
            panic!("not an artifact");
        };
        assert_eq!(text.as_deref(), Some("new"));
    }

    #[test]
    fn agent_message_maps_with_a_task_scoped_key_and_learns_the_task() {
        let mut m = StreamMapper::default();
        let mut message = msg("m-9", Role::Agent, "hello");
        message.task_id = Some(T.into());
        message.context_id = Some(C.into());
        let env = only(m.map(StreamResponse::Message(message)));
        assert_eq!(env.task_id, T);
        assert_eq!(env.key, IdemKey::Task("a2a:msg:m-9".into()));
        assert_eq!(
            env.update,
            Some(AgentUpdate::Message {
                message_id: "m-9".into(),
                text: "hello".into(),
                is_final: true,
                purpose: None
            })
        );
    }

    #[test]
    fn message_without_task_uses_the_streams_task_or_is_unsupported() {
        let mut m = StreamMapper::default();
        let bare = msg("m-1", Role::Agent, "hi");
        let out = m.map(StreamResponse::Message(bare.clone()));
        assert!(matches!(out.as_slice(), [Err(AgentError::Unsupported(_))]));

        let _ = m.map(status_update(TaskState::Working, None));
        let env = only(m.map(StreamResponse::Message(bare)));
        assert_eq!(env.task_id, T);
        assert_eq!(env.context_id, C);
    }

    #[test]
    fn user_messages_and_empty_agent_messages_are_ignored() {
        let mut m = StreamMapper::default();
        assert!(
            m.map(StreamResponse::Message(msg("u", Role::User, "x")))
                .is_empty()
        );
        let empty = Message::new(Role::Agent, vec![Part::url("https://x")]);
        assert!(m.map(StreamResponse::Message(empty)).is_empty());
    }

    #[test]
    fn task_snapshot_lists_artifacts_then_the_status_with_stream_keys() {
        let t = task(
            TaskState::Completed,
            vec![art("a-1", Some("r"), vec![Part::text("x")])],
            None,
        );
        let envs = task_envelopes(&t);
        assert_eq!(envs.len(), 2, "history is not replayed");
        assert_eq!(envs[0].key, IdemKey::Task("a2a:task-1:artifact:a-1".into()));
        assert_eq!(envs[1].key, IdemKey::Turn("task-1:status:completed".into()));
        // The same status as a live update has the same key.
        let live = only(StreamMapper::default().map(status_update(TaskState::Completed, None)));
        assert_eq!(live.key, envs[1].key);
    }

    #[test]
    fn snapshot_merges_chunked_artifacts_that_share_an_id() {
        let t = task(
            TaskState::Working,
            vec![
                art("a-6", Some("log"), vec![Part::text("one")]),
                art("a-7", Some("other"), vec![Part::text("z")]),
                art("a-6", None, vec![Part::text("two")]),
            ],
            None,
        );
        let envs = task_envelopes(&t);
        assert_eq!(envs.len(), 3);
        let Some(AgentUpdate::Artifact { text, name, .. }) = &envs[0].update else {
            panic!("not an artifact");
        };
        assert_eq!(name, "log");
        assert_eq!(text.as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn a_task_frame_teaches_the_stream_its_task() {
        let mut m = StreamMapper::default();
        let envs = m.map(StreamResponse::Task(task(
            TaskState::Submitted,
            vec![],
            None,
        )));
        assert_eq!(envs.len(), 1);
        let env = only(m.map(StreamResponse::Message(msg("m", Role::Agent, "yo"))));
        assert_eq!(env.task_id, T);
    }

    #[test]
    fn snapshot_reports_state_revision_and_rejects_unspecified() {
        let mut t = task(
            TaskState::InputRequired,
            vec![],
            Some(msg("q", Role::Agent, "why?")),
        );
        t.metadata = Some(HashMap::from([(
            RELEASE_CHANNELS_URI.to_owned(),
            json!({"requested": "staging", "revision": "coder-r51"}),
        )]));
        let s = snapshot(&t).unwrap();
        assert_eq!(s.state, AgentTaskState::InputRequired);
        assert_eq!(s.revision.as_deref(), Some("coder-r51"));
        assert_eq!(s.task_id, T);
        assert_eq!(
            s.envelopes.last().unwrap().revision.as_deref(),
            Some("coder-r51")
        );

        let bad = task(TaskState::Unspecified, vec![], None);
        assert!(matches!(snapshot(&bad), Err(AgentError::Protocol { .. })));
    }

    #[test]
    fn revision_is_read_from_event_metadata() {
        let mut u = TaskStatusUpdateEvent {
            task_id: T.into(),
            context_id: C.into(),
            status: status(TaskState::Working, None),
            metadata: Some(HashMap::from([(
                RELEASE_CHANNELS_URI.to_owned(),
                json!({"revision": "coder-r47"}),
            )])),
        };
        let env = only(StreamMapper::default().map(StreamResponse::StatusUpdate(u.clone())));
        assert_eq!(env.revision.as_deref(), Some("coder-r47"));
        u.metadata = None;
        let env = only(StreamMapper::default().map(StreamResponse::StatusUpdate(u)));
        assert_eq!(env.revision, None);
    }

    // ------------------------------------------------------------------ A2UI (ADR 0013)

    fn surface_ops() -> Value {
        json!([
            {"version": "v0.9.1", "createSurface": {"surfaceId": "s1", "catalogId": "c"}},
            {"version": "v0.9.1", "updateComponents": {"surfaceId": "s1", "components": []}}
        ])
    }

    /// A data part the way the A2UI extension spells it: `metadata.mimeType`.
    fn ui_part(data: Value) -> Part {
        let mut p = Part::data(data);
        p.metadata = Some(HashMap::from([(
            "mimeType".to_owned(),
            json!("application/a2ui+json"),
        )]));
        p
    }

    fn ui_ops(env: &AgentEnvelope) -> &Vec<Value> {
        let Some(AgentUpdate::Ui { operations }) = &env.update else {
            panic!("not a ui update: {env:?}");
        };
        operations
    }

    fn ok(v: Vec<Result<AgentEnvelope, AgentError>>) -> Vec<AgentEnvelope> {
        v.into_iter().map(Result::unwrap).collect()
    }

    #[test]
    fn an_agent_message_with_text_and_a_surface_maps_to_two_envelopes() {
        let mut message = Message::new(
            Role::Agent,
            vec![Part::text("Here is the form"), ui_part(surface_ops())],
        );
        message.message_id = "m-1".into();
        message.task_id = Some(T.into());
        message.context_id = Some(C.into());
        let envs = ok(StreamMapper::default().map(StreamResponse::Message(message)));
        assert_eq!(envs.len(), 2);
        assert_eq!(envs[0].key, IdemKey::Task("a2a:msg:m-1".into()));
        assert!(matches!(envs[0].update, Some(AgentUpdate::Message { .. })));
        assert_eq!(envs[1].key, IdemKey::Task("a2a:msg:m-1:ui:1".into()));
        assert_eq!(envs[1].task_id, T);
        assert_eq!(envs[1].task_state, None);
        assert_eq!(Value::Array(ui_ops(&envs[1]).clone()), surface_ops());
    }

    #[test]
    fn a_message_of_only_a_surface_has_no_text_envelope() {
        let mut message = Message::new(Role::Agent, vec![ui_part(surface_ops())]);
        message.message_id = "m-2".into();
        message.task_id = Some(T.into());
        let envs = ok(StreamMapper::default().map(StreamResponse::Message(message)));
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].key, IdemKey::Task("a2a:msg:m-2:ui:0".into()));
    }

    #[test]
    fn a_surface_message_without_a_task_is_unsupported_like_any_message() {
        let mut m = StreamMapper::default();
        let mut message = Message::new(Role::Agent, vec![ui_part(surface_ops())]);
        message.message_id = "m-3".into();
        let out = m.map(StreamResponse::Message(message));
        assert!(matches!(out.as_slice(), [Err(AgentError::Unsupported(_))]));
    }

    #[test]
    fn a_user_message_with_a_surface_is_ignored() {
        let message = Message::new(Role::User, vec![ui_part(surface_ops())]);
        assert!(
            StreamMapper::default()
                .map(StreamResponse::Message(message))
                .is_empty()
        );
    }

    #[test]
    fn a_status_message_surface_comes_before_the_status() {
        let mut m = msg("sm-1", Role::Agent, "Fill this in");
        m.parts.push(ui_part(surface_ops()));
        let envs =
            ok(StreamMapper::default().map(status_update(TaskState::InputRequired, Some(m))));
        assert_eq!(envs.len(), 2);
        assert_eq!(
            envs[0].key,
            IdemKey::Task("a2a:task-1:status-msg:sm-1:ui:1".into())
        );
        assert!(matches!(envs[0].update, Some(AgentUpdate::Ui { .. })));
        assert_eq!(envs[0].task_state, None);
        assert_eq!(envs[1].task_state, Some(AgentTaskState::InputRequired));
        assert_eq!(
            envs[1].update,
            Some(AgentUpdate::Status {
                state: AgentTaskState::InputRequired,
                detail: Some("Fill this in".into())
            }),
            "the detail is the text only, never the A2UI JSON"
        );
    }

    #[test]
    fn a_status_message_without_an_id_uses_a_turn_key() {
        let mut m = Message::new(Role::Agent, vec![ui_part(surface_ops())]);
        m.message_id.clear();
        let envs = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(
            envs[0].key,
            IdemKey::Turn("task-1:status-ui:working:0".into())
        );
    }

    #[test]
    fn an_artifact_of_only_a_surface_maps_to_no_artifact() {
        let a = art("a-ui", Some("form"), vec![ui_part(surface_ops())]);
        let envs = ok(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert_eq!(envs.len(), 1, "{envs:?}");
        assert_eq!(
            envs[0].key,
            IdemKey::Task("a2a:task-1:artifact:a-ui:ui:0".into())
        );
        assert!(matches!(envs[0].update, Some(AgentUpdate::Ui { .. })));
    }

    #[test]
    fn a_mixed_artifact_keeps_its_text_and_never_prints_the_surface_as_json() {
        let a = art(
            "a-mix",
            Some("report"),
            vec![Part::text("summary"), ui_part(surface_ops())],
        );
        let envs = ok(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert_eq!(envs.len(), 2);
        assert_eq!(
            envs[0].key,
            IdemKey::Task("a2a:task-1:artifact:a-mix".into())
        );
        assert_eq!(
            envs[0].update,
            Some(AgentUpdate::Artifact {
                name: "report".into(),
                mime_type: None,
                uri: None,
                text: Some("summary".into()),
            })
        );
        assert_eq!(
            envs[1].key,
            IdemKey::Task("a2a:task-1:artifact:a-mix:ui:1".into())
        );
    }

    #[test]
    fn a_chunked_surface_is_emitted_once_and_matches_the_snapshot() {
        let mut m = StreamMapper::default();
        let first = art("a-c", Some("form"), vec![Part::text("intro")]);
        assert!(m.map(artifact_update(first, None, None)).is_empty());
        let more = art("a-c", None, vec![ui_part(surface_ops())]);
        let live = ok(m.map(artifact_update(more, Some(true), Some(true))));
        let snap = task_envelopes(&task(
            TaskState::Working,
            vec![
                art("a-c", Some("form"), vec![Part::text("intro")]),
                art("a-c", None, vec![ui_part(surface_ops())]),
            ],
            None,
        ));
        let keys = |v: &[AgentEnvelope]| v.iter().map(|e| e.key.clone()).collect::<Vec<_>>();
        assert_eq!(
            keys(&live),
            keys(&snap[..2]),
            "a poll and the stream collapse"
        );
    }

    #[test]
    fn a_task_snapshot_carries_surfaces_of_artifacts_and_of_the_status_message() {
        let mut sm = msg("sm-9", Role::Agent, "pick");
        sm.parts.push(ui_part(surface_ops()));
        let t = task(
            TaskState::InputRequired,
            vec![art("a-1", None, vec![ui_part(surface_ops())])],
            Some(sm),
        );
        let envs = task_envelopes(&t);
        let kinds: Vec<&str> = envs
            .iter()
            .map(|e| match &e.update {
                Some(AgentUpdate::Ui { .. }) => "ui",
                Some(AgentUpdate::Status { .. }) => "status",
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, ["ui", "ui", "status"]);
        assert_eq!(snapshot(&t).unwrap().envelopes.len(), 3);
    }

    #[test]
    fn a_part_that_fails_the_envelope_check_is_refused_and_never_passed_on() {
        let big = json!([{"version": "v0.9.1", "updateDataModel": {
            "surfaceId": "s", "value": "x".repeat(orch_core::MAX_OPERATIONS_BYTES)}}]);
        let cases: Vec<(Part, &str)> = vec![
            (ui_part(json!({"version": "v0.9.1"})), "not an array"),
            (ui_part(json!([])), "no A2UI messages"),
            (
                ui_part(json!([{"createSurface": {"surfaceId": "s"}}])),
                "no version",
            ),
            (
                ui_part(json!([{"version": "v0.8", "createSurface": {"surfaceId": "s"}}])),
                "unsupported version",
            ),
            (
                ui_part(json!([{"version": "v0.9.1", "explode": {"surfaceId": "s"}}])),
                "unsupported operation",
            ),
            (ui_part(big), "more than the limit"),
            (
                {
                    let mut p = Part::text("[]");
                    p.metadata = Some(HashMap::from([(
                        "mimeType".to_owned(),
                        json!("application/a2ui+json"),
                    )]));
                    p
                },
                "must be a data part",
            ),
        ];
        for (part, why) in cases {
            let a = art("a-bad", Some("form"), vec![part]);
            let envs = ok(StreamMapper::default().map(artifact_update(a, None, Some(true))));
            assert_eq!(envs.len(), 1, "{why}: {envs:?}");
            let Some(AgentUpdate::UiRejected { reason }) = &envs[0].update else {
                panic!("{why}: {envs:?}");
            };
            assert!(reason.contains(why), "{why}: {reason}");
        }
    }

    #[test]
    fn a_refusal_in_one_part_does_not_stop_the_next() {
        let mut m = msg("m-mix", Role::Agent, "two parts");
        m.task_id = Some(T.into());
        m.parts.push(ui_part(json!("bad")));
        m.parts.push(ui_part(surface_ops()));
        let envs = ok(StreamMapper::default().map(StreamResponse::Message(m)));
        assert_eq!(envs.len(), 3);
        assert!(matches!(
            envs[1].update,
            Some(AgentUpdate::UiRejected { .. })
        ));
        assert!(matches!(envs[2].update, Some(AgentUpdate::Ui { .. })));
    }

    #[test]
    fn the_media_type_is_recognised_in_both_spellings_and_only_that() {
        // A2A 1.0 `mediaType`, with a parameter and other case.
        let by_media_type =
            Part::data(surface_ops()).with_media_type("Application/A2UI+JSON; charset=utf-8");
        assert!(claims_a2ui(&by_media_type));
        assert!(claims_a2ui(&ui_part(surface_ops())));
        // Another JSON data part is an ordinary artifact, exactly as before.
        let other = Part::data(json!({"ok": true})).with_media_type("application/json");
        assert!(!claims_a2ui(&other));
        let mut wrong = Part::data(surface_ops());
        wrong.metadata = Some(HashMap::from([(
            "mimeType".to_owned(),
            json!("text/plain"),
        )]));
        assert!(!claims_a2ui(&wrong));
        let a = art("a-json", Some("data"), vec![other]);
        let envs = ok(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert!(matches!(envs[0].update, Some(AgentUpdate::Artifact { .. })));
        assert_eq!(envs.len(), 1);
        // A surface in a data part of another type is not relayed as UI.
        let a = art("a-x", None, vec![wrong]);
        let envs = ok(StreamMapper::default().map(artifact_update(a, None, Some(true))));
        assert!(matches!(envs[0].update, Some(AgentUpdate::Artifact { .. })));
    }
    // ---- steps (ADR 0025) ---------------------------------------------------------------

    fn step_message(id: &str, entry: Value) -> Message {
        let mut m = msg(id, Role::Agent, "npm test");
        m.metadata = Some(HashMap::from([(STEPS_EXTENSION.to_owned(), entry)]));
        m
    }

    fn step_entry() -> Value {
        json!({"id": "acp:call_2:toolu_01", "parentId": "tool:call_2", "kind": "command",
               "label": "npm test", "state": "running", "icon": "execute",
               "detail": "12 passed, 1 failed"})
    }

    fn step_of_envelope(env: &AgentEnvelope) -> &StepReport {
        let Some(AgentUpdate::Step(step)) = &env.update else {
            panic!("not a step: {env:?}");
        };
        step
    }

    #[test]
    fn a_working_status_with_a_step_is_a_step_with_ids_made_unique_by_the_task() {
        let m = step_message("sm-1", step_entry());
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(env.task_state, Some(AgentTaskState::Working));
        assert_eq!(
            step_of_envelope(&env),
            &StepReport {
                id: "task-1/acp:call_2:toolu_01".into(),
                parent: Some("task-1/tool:call_2".into()),
                kind: StepKind::Command,
                label: "npm test".into(),
                state: StepState::Running,
                icon: Some("execute".into()),
                detail: Some("12 passed, 1 failed".into()),
                input: None,
                output: None,
            }
        );
        assert_eq!(
            env.key,
            IdemKey::Task("a2a:task-1:step:acp:call_2:toolu_01:sm-1".into()),
            "the key names the agent's id and the status message"
        );
    }

    #[test]
    fn the_opencode_icon_of_a_step_that_hands_work_to_opencode_survives_the_door() {
        // adam-rs names the step that hands work to OpenCode over ACP `opencode` (ADR 0049)
        let mut entry = step_entry();
        entry["icon"] = json!("opencode");
        entry["label"] = json!("OpenCode: add the retry loop");
        let m = step_message("sm-9", entry);
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        let step = step_of_envelope(&env);
        assert_eq!(step.icon.as_deref(), Some("opencode"));
        let kept = step.sanitize(orch_core::StepSource::Agent).unwrap();
        assert_eq!(kept.icon.as_deref(), Some("opencode"));
        // a name that only starts like it is outside the vocabulary
        let mut near = step.clone();
        near.icon = Some("opencode-pro".into());
        assert_eq!(
            near.sanitize(orch_core::StepSource::Agent).unwrap().icon,
            None
        );
    }

    #[test]
    fn a_steps_input_and_output_are_read_and_an_agent_that_sends_none_still_works() {
        // the members of ADR 0030, as an agent sends them
        let mut entry = step_entry();
        entry["input"] = json!({"query": "node 24", "limit": 3});
        entry["output"] =
            json!({"text": "two results", "truncated": true, "bytes": 90_000, "error": true});
        let m = step_message("sm-1", entry);
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        let step = step_of_envelope(&env);
        assert_eq!(
            step.input.as_ref().map(|i| Value::Object(i.clone())),
            Some(json!({"query": "node 24", "limit": 3}))
        );
        assert_eq!(
            step.output,
            Some(StepOutput {
                text: "two results".into(),
                truncated: true,
                bytes: Some(90_000),
                error: true,
            })
        );
        // an agent that sends neither (every agent before ADR 0030)
        let m = step_message("sm-2", step_entry());
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        let step = step_of_envelope(&env);
        assert_eq!((&step.input, &step.output), (&None, &None));
        // an output that only has its text
        let mut entry = step_entry();
        entry["output"] = json!({"text": "ok"});
        let m = step_message("sm-3", entry);
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(
            step_of_envelope(&env).output,
            Some(StepOutput {
                text: "ok".into(),
                ..StepOutput::default()
            })
        );
    }

    #[test]
    fn a_bad_input_or_output_drops_only_that_member_and_never_the_step() {
        let cases = [
            ("input", json!("npm test")),
            ("input", json!(["npm", "test"])),
            ("input", json!(null)),
            ("input", json!(7)),
            ("output", json!("two results")),
            ("output", json!(["two results"])),
            ("output", json!({"text": 5})),
            ("output", json!({"text": null})),
            ("output", json!({"truncated": true})),
            ("output", json!(null)),
        ];
        for (member, value) in cases {
            let mut entry = step_entry();
            entry[member] = value.clone();
            let m = step_message("sm-1", entry);
            let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
            let step = step_of_envelope(&env);
            assert_eq!(step.label, "npm test", "{member} {value}: the step stays");
            assert_eq!(step.detail.as_deref(), Some("12 passed, 1 failed"));
            assert!(
                step.input.is_none() && step.output.is_none(),
                "{member} {value}: dropped"
            );
        }
        // a bad input does not take a good output with it, and the other way round
        let mut entry = step_entry();
        entry["input"] = json!("npm test");
        entry["output"] = json!({"text": "fine"});
        let env = only(StreamMapper::default().map(status_update(
            TaskState::Working,
            Some(step_message("sm-1", entry)),
        )));
        let step = step_of_envelope(&env);
        assert!(step.input.is_none());
        assert_eq!(step.output.as_ref().map(|o| o.text.as_str()), Some("fine"));
        // an optional member of an output that has the wrong type is read as absent
        let mut entry = step_entry();
        entry["output"] = json!({"text": "fine", "truncated": "yes", "bytes": -1, "error": 1});
        let env = only(StreamMapper::default().map(status_update(
            TaskState::Working,
            Some(step_message("sm-2", entry)),
        )));
        assert_eq!(
            step_of_envelope(&env).output,
            Some(StepOutput {
                text: "fine".into(),
                ..StepOutput::default()
            })
        );
    }

    #[test]
    fn the_status_text_of_a_step_is_not_a_status_of_its_own() {
        let m = step_message("sm-1", step_entry());
        let envs = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(
            envs.len(),
            1,
            "the label is the step's, not a second `working`"
        );
    }

    #[test]
    fn only_the_required_members_are_required_and_a_kind_that_is_unknown_is_a_tool() {
        let m = step_message(
            "sm-1",
            json!({"id": "a", "label": "x", "state": "waiting", "kind": "robot"}),
        );
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        let step = step_of_envelope(&env);
        assert_eq!(
            (
                step.kind,
                step.state,
                &step.parent,
                &step.icon,
                &step.detail
            ),
            (StepKind::Tool, StepState::Waiting, &None, &None, &None)
        );
        // no kind at all is a tool too
        let m = step_message(
            "sm-2",
            json!({"id": "a", "label": "x", "state": "completed"}),
        );
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(step_of_envelope(&env).kind, StepKind::Tool);
        // `null` for an optional member is absent
        let m = step_message(
            "sm-3",
            json!({"id": "a", "label": "x", "state": "failed", "parentId": null, "icon": null, "detail": null}),
        );
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(step_of_envelope(&env).parent, None);
    }

    #[test]
    fn an_entry_that_does_not_validate_is_read_as_a_plain_status() {
        let valid = step_entry();
        let broken = |f: &dyn Fn(&mut serde_json::Map<String, Value>)| {
            let mut entry = valid.as_object().unwrap().clone();
            f(&mut entry);
            Value::Object(entry)
        };
        let cases = [
            broken(&|e| {
                e.remove("id");
            }),
            broken(&|e| {
                e.remove("label");
            }),
            broken(&|e| {
                e.remove("state");
            }),
            broken(&|e| {
                e.insert("id".into(), json!(""));
            }),
            broken(&|e| {
                e.insert("id".into(), json!("x".repeat(129)));
            }),
            broken(&|e| {
                e.insert("id".into(), json!("a\nb"));
            }),
            broken(&|e| {
                e.insert("id".into(), json!(7));
            }),
            broken(&|e| {
                e.insert("label".into(), json!("   "));
            }),
            broken(&|e| {
                e.insert("label".into(), json!(["npm", "test"]));
            }),
            broken(&|e| {
                e.insert("state".into(), json!("paused"));
            }),
            broken(&|e| {
                e.insert("state".into(), json!("Running"));
            }),
            broken(&|e| {
                e.insert("kind".into(), json!(3));
            }),
            broken(&|e| {
                e.insert("parentId".into(), json!({"id": "x"}));
            }),
            broken(&|e| {
                e.insert("icon".into(), json!(true));
            }),
            broken(&|e| {
                e.insert("detail".into(), json!(1.5));
            }),
            json!("a string, not an entry"),
            json!(["id"]),
        ];
        for entry in cases {
            let m = step_message("sm-1", entry.clone());
            let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
            assert_eq!(
                env.update,
                Some(AgentUpdate::Status {
                    state: AgentTaskState::Working,
                    detail: Some("npm test".into())
                }),
                "{entry}"
            );
            assert_eq!(env.key, IdemKey::Task("a2a:task-1:status-msg:sm-1".into()));
        }
    }

    #[test]
    fn a_step_on_a_status_that_is_not_working_is_ignored() {
        for state in [
            TaskState::InputRequired,
            TaskState::AuthRequired,
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Canceled,
            TaskState::Rejected,
            TaskState::Submitted,
        ] {
            let m = step_message("sm-1", step_entry());
            let env = only(StreamMapper::default().map(status_update(state.clone(), Some(m))));
            assert!(
                !matches!(env.update, Some(AgentUpdate::Step(_))),
                "{state:?}: the state ends or pauses the turn, a step cannot say that"
            );
        }
    }

    #[test]
    fn only_the_exact_uri_counts() {
        for near in [
            "https://agents.vymalo.com/a2a/extensions/steps/v2",
            "https://agents.vymalo.com/a2a/extensions/steps/v1/",
            "http://agents.vymalo.com/a2a/extensions/steps/v1",
            "https://agents.vymalo.com/a2a/extensions/Steps/v1",
        ] {
            let mut m = msg("sm-1", Role::Agent, "npm test");
            m.metadata = Some(HashMap::from([(near.to_owned(), step_entry())]));
            let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
            assert!(
                matches!(env.update, Some(AgentUpdate::Status { .. })),
                "{near}"
            );
        }
    }

    #[test]
    fn a_status_message_without_an_id_uses_a_turn_key_that_names_the_step_and_its_state() {
        let mut m = step_message("", step_entry());
        m.message_id.clear();
        let env = only(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(
            env.key,
            IdemKey::Turn("task-1:step:acp:call_2:toolu_01:running".into())
        );
    }

    #[test]
    fn a_surface_in_the_message_of_a_step_comes_first_and_the_step_after() {
        let mut m = step_message("sm-1", step_entry());
        m.parts.push(ui_part(surface_ops()));
        let envs = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(envs.len(), 2);
        assert!(matches!(envs[0].update, Some(AgentUpdate::Ui { .. })));
        assert!(matches!(envs[1].update, Some(AgentUpdate::Step(_))));
    }

    #[test]
    fn a_poll_maps_a_step_to_the_keys_of_the_stream() {
        let m = step_message("sm-1", step_entry());
        let live =
            only(StreamMapper::default().map(status_update(TaskState::Working, Some(m.clone()))));
        let snap = snapshot(&task(TaskState::Working, vec![], Some(m))).unwrap();
        let last = snap.envelopes.last().unwrap();
        assert_eq!(last.key, live.key);
        assert_eq!(last.update, live.update);
        assert_eq!(snap.state, AgentTaskState::Working);
    }

    #[test]
    fn the_revision_of_the_event_goes_with_the_step() {
        let m = step_message("sm-1", step_entry());
        let mut event = status_update(TaskState::Working, Some(m));
        if let StreamResponse::StatusUpdate(u) = &mut event {
            u.metadata = Some(HashMap::from([(
                RELEASE_CHANNELS_URI.to_owned(),
                json!({"requested": "stable", "revision": "rev-9"}),
            )]));
        }
        let env = only(StreamMapper::default().map(event));
        assert_eq!(env.revision.as_deref(), Some("rev-9"));
        assert!(matches!(env.update, Some(AgentUpdate::Step(_))));
    }
    // ---- streamed text (ADR 0027) --------------------------------------------------------

    /// A chunk of stream `id`: one text part, the entry under the URI.
    fn chunk(id: &str, offset: u64, text: &str, append: bool, last: bool) -> StreamResponse {
        let mut a = art(id, Some("reply"), vec![Part::text(text)]);
        a.extensions = Some(vec![TEXT_STREAM_EXTENSION.to_owned()]);
        a.metadata = Some(HashMap::from([(
            TEXT_STREAM_EXTENSION.to_owned(),
            json!({"offset": offset}),
        )]));
        artifact_update(a, Some(append), Some(last))
    }

    /// A status message that states the whole text of stream `id`.
    fn marked(message_id: &str, stream: Value, text: &str) -> Message {
        let mut m = msg(message_id, Role::Agent, text);
        m.metadata = Some(HashMap::from([(
            TEXT_STREAM_EXTENSION.to_owned(),
            json!({"streamId": stream}),
        )]));
        m
    }

    fn live_of(env: &AgentEnvelope) -> &LiveChunk {
        env.live
            .as_ref()
            .unwrap_or_else(|| panic!("not a live chunk: {env:?}"))
    }

    #[test]
    fn a_chunk_is_a_live_piece_that_is_never_applied_and_never_an_artifact() {
        let mut mapper = StreamMapper::default();
        let first = only(mapper.map(chunk("S", 0, "Fib", false, false)));
        assert_eq!(
            live_of(&first),
            &LiveChunk {
                message_id: "S".into(),
                offset: 0,
                text: "Fib".into(),
                end: LiveEnd::Open,
                kind: orch_core::LiveKind::Reply,
            }
        );
        assert_eq!(first.update, None, "nothing of it is applied");
        assert_eq!(first.task_state, None);
        assert_eq!((first.task_id.as_str(), first.context_id.as_str()), (T, C));
        assert_eq!(first.key, IdemKey::Turn("task-1:live:S:0".into()));
        // The next chunk is its own piece at once, not held back to be assembled.
        let second = only(mapper.map(chunk("S", 3, "onacci ", true, false)));
        assert_eq!(live_of(&second).offset, 3);
        assert_eq!(second.key, IdemKey::Turn("task-1:live:S:3".into()));
        let last = only(mapper.map(chunk("S", 10, "in Rust.", true, true)));
        assert_eq!(live_of(&last).end, LiveEnd::Last);
        // Nothing is left to assemble: the next event brings no artifact.
        let status = only(mapper.map(status_update(TaskState::Completed, None)));
        assert!(matches!(status.update, Some(AgentUpdate::Status { .. })));
    }

    #[test]
    fn a_whole_offset_may_be_written_as_a_float_as_the_a2a_sdk_does() {
        // metadata is a protobuf Struct: `10` comes back as `10.0`
        for (written, want) in [(json!(10), 10), (json!(10.0), 10), (json!(0.0), 0)] {
            let mut event = chunk("S", 0, "x", true, false);
            if let StreamResponse::ArtifactUpdate(u) = &mut event {
                u.artifact.metadata = Some(HashMap::from([(
                    TEXT_STREAM_EXTENSION.to_owned(),
                    json!({"offset": written}),
                )]));
            }
            let env = only(StreamMapper::default().map(event));
            assert_eq!(live_of(&env).offset, want);
        }
    }

    #[test]
    fn the_end_of_a_chunk_is_the_last_one_or_the_agent_giving_up() {
        let end_of = |event: StreamResponse| live_of(&only(StreamMapper::default().map(event))).end;
        assert_eq!(end_of(chunk("S", 0, "a", false, false)), LiveEnd::Open);
        assert_eq!(end_of(chunk("S", 0, "a", false, true)), LiveEnd::Last);
        let abandoned = |last: bool| {
            let mut event = chunk("S", 5, "", true, last);
            if let StreamResponse::ArtifactUpdate(u) = &mut event {
                u.artifact.metadata = Some(HashMap::from([(
                    TEXT_STREAM_EXTENSION.to_owned(),
                    json!({"offset": 5, "abandoned": true}),
                )]));
            }
            event
        };
        assert_eq!(end_of(abandoned(true)), LiveEnd::Abandoned);
        // Said without `lastChunk`, giving up ends the stream too.
        assert_eq!(end_of(abandoned(false)), LiveEnd::Abandoned);
    }

    #[test]
    fn offsets_are_utf8_bytes_as_sent_and_the_text_is_kept_whole() {
        let env =
            only(StreamMapper::default().map(chunk("S", 6, "\u{4e2d}x\u{1f980}", true, false)));
        let live = live_of(&env);
        assert_eq!(live.offset, 6);
        assert_eq!(live.text, "\u{4e2d}x\u{1f980}");
        // The last chunk may say nothing.
        let empty = only(StreamMapper::default().map(chunk("S", 8, "", true, true)));
        assert_eq!(live_of(&empty).text, "");
    }

    #[test]
    fn a_live_chunk_ends_an_artifact_that_was_held_back() {
        let mut mapper = StreamMapper::default();
        // A plain chunked artifact is held until the next event shows nothing more follows.
        assert!(
            mapper
                .map(artifact_update(
                    art("a-1", Some("log"), vec![Part::text("x")]),
                    None,
                    Some(false)
                ))
                .is_empty()
        );
        let out = ok(mapper.map(chunk("S", 0, "Fib", false, false)));
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0].key, IdemKey::Task("a2a:task-1:artifact:a-1".into()));
        assert!(matches!(out[0].update, Some(AgentUpdate::Artifact { .. })));
        assert!(out[1].live.is_some());
    }

    #[test]
    fn an_entry_that_does_not_validate_is_the_plain_artifact_chunk_it_was() {
        let entry_of = |entry: Value| {
            let mut event = chunk("S", 0, "Fib", false, false);
            if let StreamResponse::ArtifactUpdate(u) = &mut event {
                u.artifact.metadata =
                    Some(HashMap::from([(TEXT_STREAM_EXTENSION.to_owned(), entry)]));
            }
            event
        };
        let with_id = |id: &str| {
            let mut event = chunk("S", 0, "Fib", false, false);
            if let StreamResponse::ArtifactUpdate(u) = &mut event {
                u.artifact.artifact_id = id.to_owned();
            }
            event
        };
        let with_parts = |parts: Vec<Part>| {
            let mut event = chunk("S", 0, "Fib", false, false);
            if let StreamResponse::ArtifactUpdate(u) = &mut event {
                u.artifact.parts = parts;
            }
            event
        };
        let broken = [
            ("no offset", entry_of(json!({}))),
            ("a negative offset", entry_of(json!({"offset": -1}))),
            ("a fractional offset", entry_of(json!({"offset": 1.5}))),
            ("a negative whole offset", entry_of(json!({"offset": -2.0}))),
            (
                "an offset too large to be exact",
                entry_of(json!({"offset": 1.0e19})),
            ),
            ("a text offset", entry_of(json!({"offset": "0"}))),
            ("an entry that is not an object", entry_of(json!(7))),
            ("an empty stream id", with_id("")),
            ("a stream id over 128 bytes", with_id(&"s".repeat(129))),
            ("a control character in the id", with_id("a\nb")),
            ("no part", with_parts(vec![])),
            (
                "two parts",
                with_parts(vec![Part::text("a"), Part::text("b")]),
            ),
            ("a data part", with_parts(vec![Part::data(json!({"a": 1}))])),
            (
                "a url part",
                with_parts(vec![Part::url("https://example.com/x")]),
            ),
        ];
        for (what, event) in broken {
            let mut mapper = StreamMapper::default();
            // Held back like any artifact chunk, then emitted once by the event that follows.
            assert!(
                mapper.map(event).is_empty(),
                "{what}: a chunk was made of it"
            );
            let out = ok(mapper.map(status_update(TaskState::Working, None)));
            assert!(
                matches!(out[0].update, Some(AgentUpdate::Artifact { .. }))
                    && out[0].live.is_none(),
                "{what}: {out:?}"
            );
        }
        // The stream id of exactly 128 bytes passes.
        let env = only(StreamMapper::default().map(with_id(&"s".repeat(128))));
        assert_eq!(live_of(&env).message_id.len(), 128);
    }

    #[test]
    fn only_the_exact_uri_is_a_chunk() {
        for near in [
            "https://agents.vymalo.com/a2a/extensions/text-stream/v2",
            "https://agents.vymalo.com/a2a/extensions/text-stream/v1/",
            "https://agents.vymalo.com/a2a/extensions/steps/v1",
        ] {
            let mut event = chunk("S", 0, "Fib", false, false);
            if let StreamResponse::ArtifactUpdate(u) = &mut event {
                u.artifact.metadata =
                    Some(HashMap::from([(near.to_owned(), json!({"offset": 0}))]));
            }
            assert!(StreamMapper::default().map(event).is_empty(), "{near}");
        }
    }

    #[test]
    fn a_task_never_shows_a_chunk_as_an_artifact() {
        let kept = match chunk("S", 0, "Fib", false, true) {
            StreamResponse::ArtifactUpdate(u) => u.artifact,
            other => panic!("{other:?}"),
        };
        let plain = art("a-1", Some("log"), vec![Part::text("x")]);
        let snap = snapshot(&task(TaskState::Working, vec![kept, plain], None)).unwrap();
        let artifacts: Vec<&IdemKey> = snap
            .envelopes
            .iter()
            .filter(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
            .map(|e| &e.key)
            .collect();
        assert_eq!(
            artifacts,
            [&IdemKey::Task("a2a:task-1:artifact:a-1".into())]
        );
    }

    #[test]
    fn a_marked_working_status_is_the_message_then_a_status_without_words() {
        let m = marked("sm-1", json!("S"), "Let me look at the repository.");
        let envs = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(envs.len(), 2, "{envs:?}");
        assert_eq!(envs[0].key, IdemKey::Task("a2a:msg:S".into()));
        assert_eq!(envs[0].task_state, None);
        assert_eq!(
            envs[0].update,
            Some(AgentUpdate::Message {
                message_id: "S".into(),
                text: "Let me look at the repository.".into(),
                is_final: true,
                purpose: Some(MessagePurpose::Working)
            })
        );
        assert_eq!(
            envs[1].key,
            IdemKey::Task("a2a:task-1:status-msg:sm-1".into())
        );
        assert_eq!(
            envs[1].update,
            Some(AgentUpdate::Status {
                state: AgentTaskState::Working,
                detail: None
            }),
            "its words are the message"
        );
        assert!(envs.iter().all(|e| e.live.is_none()));
    }

    #[test]
    fn a_marked_status_that_ends_the_turn_keeps_its_words() {
        for (state, want) in [
            (TaskState::Completed, AgentTaskState::Completed),
            (TaskState::InputRequired, AgentTaskState::InputRequired),
            (TaskState::AuthRequired, AgentTaskState::AuthRequired),
        ] {
            let m = marked("sm-2", json!("S"), "Fibonacci in Rust.");
            let envs = ok(StreamMapper::default().map(status_update(state, Some(m))));
            assert_eq!(envs.len(), 2);
            assert!(matches!(
                &envs[0].update,
                Some(AgentUpdate::Message { message_id, purpose, .. })
                    if message_id == "S" && *purpose == Some(MessagePurpose::Answer)
            ));
            assert_eq!(
                envs[1].update,
                Some(AgentUpdate::Status {
                    state: want,
                    detail: Some("Fibonacci in Rust.".into())
                }),
                "the interrupt and the verifier's summary read it"
            );
        }
    }

    #[test]
    fn stated_words_are_marked_by_the_status_they_came_on() {
        let purpose = |state| {
            let m = marked("sm-4", json!("S"), "Some words.");
            let envs = ok(StreamMapper::default().map(status_update(state, Some(m))));
            match &envs[0].update {
                Some(AgentUpdate::Message { purpose, .. }) => *purpose,
                other => panic!("not a message: {other:?}"),
            }
        };
        // a `working` status: said while the agent goes on; a status that ends the turn: the answer
        assert_eq!(purpose(TaskState::Working), Some(MessagePurpose::Working));
        assert_eq!(purpose(TaskState::Completed), Some(MessagePurpose::Answer));
        assert_eq!(
            purpose(TaskState::InputRequired),
            Some(MessagePurpose::Answer)
        );
        assert_eq!(
            purpose(TaskState::AuthRequired),
            Some(MessagePurpose::Answer)
        );
        // the words of a failure are its error, not an answer; nothing is said about them
        assert_eq!(purpose(TaskState::Failed), None);
        assert_eq!(purpose(TaskState::Canceled), None);
        assert_eq!(purpose(TaskState::Rejected), None);
    }

    #[test]
    fn a_plain_message_is_not_marked_whatever_the_task_is_doing() {
        // an agent `Message` has no status to read a purpose from, and one that carries the
        // stream marker in its metadata is still only a message
        let mut m = StreamMapper::default();
        let mut message = msg("m-10", Role::Agent, "hello");
        message.task_id = Some(T.into());
        message.context_id = Some(C.into());
        message.metadata = Some(HashMap::from([(
            TEXT_STREAM_EXTENSION.to_owned(),
            json!({"streamId": "S"}),
        )]));
        let env = only(m.map(StreamResponse::Message(message)));
        assert!(matches!(
            env.update,
            Some(AgentUpdate::Message { purpose: None, .. })
        ));
    }

    #[test]
    fn a_marker_that_does_not_validate_is_a_plain_status() {
        let long = "s".repeat(129);
        let cases = [
            ("no id", json!(null)),
            ("an empty id", json!("")),
            ("an id that is a number", json!(7)),
            ("an id over 128 bytes", json!(long)),
            ("a control character", json!("a\tb")),
        ];
        for (what, stream) in cases {
            let m = marked("sm-3", stream, "Fibonacci");
            let envs = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
            assert_eq!(envs.len(), 1, "{what}: {envs:?}");
            assert_eq!(
                envs[0].update,
                Some(AgentUpdate::Status {
                    state: AgentTaskState::Working,
                    detail: Some("Fibonacci".into())
                }),
                "{what}"
            );
        }
        // No text to state, and a marker on another URI.
        let blank = marked("sm-3", json!("S"), "   ");
        let envs = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(blank))));
        assert_eq!(envs.len(), 1);
        let mut near = marked("sm-3", json!("S"), "Fib");
        near.metadata = Some(HashMap::from([(
            "https://agents.vymalo.com/a2a/extensions/text-stream/v2".to_owned(),
            json!({"streamId": "S"}),
        )]));
        let envs = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(near))));
        assert_eq!(envs.len(), 1);
    }

    #[test]
    fn a_step_is_a_step_and_its_text_is_a_label_not_a_stream() {
        let mut m = step_message("sm-1", step_entry());
        m.metadata
            .as_mut()
            .unwrap()
            .insert(TEXT_STREAM_EXTENSION.to_owned(), json!({"streamId": "S"}));
        let envs = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(m))));
        assert_eq!(envs.len(), 1, "{envs:?}");
        assert!(matches!(envs[0].update, Some(AgentUpdate::Step(_))));
    }

    #[test]
    fn a_poll_and_the_stream_say_the_whole_text_under_the_same_keys() {
        let m = marked("sm-2", json!("S"), "Fibonacci in Rust.");
        let live: Vec<AgentEnvelope> =
            ok(StreamMapper::default().map(status_update(TaskState::Completed, Some(m.clone()))));
        let snap = snapshot(&task(TaskState::Completed, vec![], Some(m))).unwrap();
        let tail = &snap.envelopes[snap.envelopes.len() - 2..];
        for (a, b) in live.iter().zip(tail) {
            assert_eq!(a.key, b.key);
            assert_eq!(a.update, b.update);
        }
        assert_eq!(live[0].key, IdemKey::Task("a2a:msg:S".into()));
    }

    #[test]
    fn the_same_stream_stated_twice_is_one_key() {
        let a = marked("sm-1", json!("S"), "Fibonacci in Rust.");
        let b = marked("sm-2", json!("S"), "Fibonacci in Rust.");
        let first = ok(StreamMapper::default().map(status_update(TaskState::Working, Some(a))));
        let second = ok(StreamMapper::default().map(status_update(TaskState::Completed, Some(b))));
        assert_eq!(first[0].key, second[0].key, "the dispatcher stores it once");
    }

    // ---- reasoning (ADR 0044) ---------------------------------------------------------------

    /// A chunk of the reasoning stream `id`: the entry says `kind: "reasoning"`.
    fn thought(id: &str, offset: u64, text: &str, last: bool) -> StreamResponse {
        let mut a = art(id, Some("reasoning"), vec![Part::text(text)]);
        a.extensions = Some(vec![TEXT_STREAM_EXTENSION.to_owned()]);
        a.metadata = Some(HashMap::from([(
            TEXT_STREAM_EXTENSION.to_owned(),
            json!({"offset": offset, "kind": "reasoning"}),
        )]));
        artifact_update(a, Some(offset > 0), Some(last))
    }

    fn abandoned_thought(id: &str, offset: u64, text: &str) -> StreamResponse {
        let mut event = thought(id, offset, text, true);
        if let StreamResponse::ArtifactUpdate(u) = &mut event {
            u.artifact.metadata = Some(HashMap::from([(
                TEXT_STREAM_EXTENSION.to_owned(),
                json!({"offset": offset, "kind": "reasoning", "abandoned": true}),
            )]));
        }
        event
    }

    /// What the reasoning an update logs says, if the envelope is one.
    fn logged(env: &AgentEnvelope) -> Option<(&str, &str, bool)> {
        match &env.update {
            Some(AgentUpdate::Reasoning {
                message_id,
                text,
                truncated,
            }) => Some((message_id.as_str(), text.as_str(), *truncated)),
            _ => None,
        }
    }

    #[test]
    fn a_reasoning_chunk_is_a_live_piece_of_its_own_kind_and_never_applied() {
        let mut mapper = StreamMapper::default();
        let first = only(mapper.map(thought("R", 0, "The user ", false)));
        assert_eq!(
            live_of(&first),
            &LiveChunk {
                message_id: "R".into(),
                offset: 0,
                text: "The user ".into(),
                end: LiveEnd::Open,
                kind: orch_core::LiveKind::Reasoning,
            }
        );
        assert_eq!(first.update, None);
        assert_eq!(first.key, IdemKey::Turn("task-1:live:R:0".into()));
        // A reply's chunk says it is a reply.
        let reply = only(StreamMapper::default().map(chunk("S", 0, "Hi", false, false)));
        assert_eq!(live_of(&reply).kind, orch_core::LiveKind::Reply);
    }

    #[test]
    fn the_last_chunk_of_a_reasoning_logs_it_whole_once_under_its_own_key() {
        let mut mapper = StreamMapper::default();
        let a = ok(mapper.map(thought("R", 0, "The user wants ", false)));
        assert_eq!(a.len(), 1, "a piece alone logs nothing");
        let b = ok(mapper.map(thought("R", 15, "Fibonacci.", false)));
        assert_eq!(b.len(), 1);
        let end = ok(mapper.map(thought("R", 25, "", true)));
        // the live piece that ends the stream, then the reasoning, whole
        assert_eq!(end.len(), 2, "{end:?}");
        assert_eq!(live_of(&end[0]).end, LiveEnd::Last);
        assert_eq!(
            logged(&end[1]),
            Some(("R", "The user wants Fibonacci.", false))
        );
        assert_eq!(end[1].key, IdemKey::Task("a2a:task-1:reasoning:R".into()));
        assert_eq!(end[1].live, None);
        // The stream is forgotten: a second stream of the same id, from the start, is logged again under the same key
        // (the dispatcher stores it once).
        let again = ok(mapper.map(thought("R", 0, "x", true)));
        assert_eq!(logged(&again[1]), Some(("R", "x", false)));
    }

    #[test]
    fn a_reply_logs_no_reasoning() {
        let mut mapper = StreamMapper::default();
        let envs = ok(mapper.map(chunk("S", 0, "Fib", false, true)));
        assert_eq!(envs.len(), 1);
        assert!(envs.iter().all(|e| logged(e).is_none()));
    }

    #[test]
    fn a_reasoning_whose_beginning_was_not_seen_is_relayed_and_not_logged() {
        // a resubscribe joined it mid-way: the whole is not held
        let mut mapper = StreamMapper::default();
        let envs = ok(mapper.map(thought("R", 9, "wants Fibonacci.", true)));
        assert_eq!(envs.len(), 1);
        assert!(envs[0].live.is_some());
    }

    #[test]
    fn overlap_is_trimmed_and_a_gap_marks_the_text_cut() {
        let mut mapper = StreamMapper::default();
        ok(mapper.map(thought("R", 0, "abcdef", false)));
        // an overlap: "def" again, then "ghi"
        ok(mapper.map(thought("R", 3, "defghi", false)));
        let end = ok(mapper.map(thought("R", 9, "", true)));
        assert_eq!(logged(&end[1]), Some(("R", "abcdefghi", false)));

        // a gap: what is held is the beginning, and it says so
        let mut mapper = StreamMapper::default();
        ok(mapper.map(thought("R", 0, "abc", false)));
        ok(mapper.map(thought("R", 10, "xyz", false)));
        let end = ok(mapper.map(thought("R", 13, "", true)));
        assert_eq!(logged(&end[1]), Some(("R", "abc", true)));
    }

    #[test]
    fn a_reasoning_the_agent_gave_up_is_logged_as_far_as_it_went_and_cut() {
        let mut mapper = StreamMapper::default();
        ok(mapper.map(thought("R", 0, "The user wants", false)));
        let end = ok(mapper.map(abandoned_thought("R", 14, "")));
        assert_eq!(live_of(&end[0]).end, LiveEnd::Abandoned);
        assert_eq!(logged(&end[1]), Some(("R", "The user wants", true)));
    }

    #[test]
    fn a_blank_reasoning_is_not_logged() {
        let mut mapper = StreamMapper::default();
        ok(mapper.map(thought("R", 0, "\n\n", false)));
        let end = ok(mapper.map(thought("R", 2, "", true)));
        assert_eq!(end.len(), 1, "only the live piece: {end:?}");
    }

    #[test]
    fn a_reasoning_is_collected_up_to_the_bound_and_says_it_was_cut() {
        let mut mapper = StreamMapper::default();
        let piece = "é".repeat(1024);
        let mut offset = 0u64;
        // far over the bound (32 KiB): 48 pieces of 2 KiB
        for _ in 0..48 {
            ok(mapper.map(thought("R", offset, &piece, false)));
            offset += piece.len() as u64;
        }
        let end = ok(mapper.map(thought("R", offset, "", true)));
        let (_, text, truncated) = logged(&end[1]).expect("logged");
        assert!(truncated);
        assert_eq!(text.len(), MAX_REASONING_BYTES);
        assert!(text.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_kind_this_reader_does_not_know_is_neither_a_reply_nor_an_artifact() {
        for kind in [json!("summary"), json!(7), json!({"a": 1})] {
            let mut event = thought("R", 0, "x", false);
            if let StreamResponse::ArtifactUpdate(u) = &mut event {
                u.artifact.metadata = Some(HashMap::from([(
                    TEXT_STREAM_EXTENSION.to_owned(),
                    json!({"offset": 0, "kind": kind}),
                )]));
            }
            let envs = ok(StreamMapper::default().map(event));
            assert!(envs.is_empty(), "{kind}: {envs:?}");
        }
        // null is no kind: a reply
        let mut event = thought("R", 0, "x", false);
        if let StreamResponse::ArtifactUpdate(u) = &mut event {
            u.artifact.metadata = Some(HashMap::from([(
                TEXT_STREAM_EXTENSION.to_owned(),
                json!({"offset": 0, "kind": null}),
            )]));
        }
        let env = only(StreamMapper::default().map(event));
        assert_eq!(live_of(&env).kind, orch_core::LiveKind::Reply);
    }

    #[test]
    fn a_snapshot_never_holds_a_reasoning_chunk() {
        let mut a = art("R", Some("reasoning"), vec![Part::text("x")]);
        a.metadata = Some(HashMap::from([(
            TEXT_STREAM_EXTENSION.to_owned(),
            json!({"offset": 0, "kind": "reasoning"}),
        )]));
        let snap = snapshot(&task(TaskState::Working, vec![a], None)).unwrap();
        assert!(
            snap.envelopes
                .iter()
                .all(|e| e.update.is_none()
                    || !matches!(e.update, Some(AgentUpdate::Artifact { .. })))
        );
    }

    // ---- an agent that answers through artifacts (kagent, ADR 0031 amendment 2026-10-07) -----------

    /// What kagent v0.10.3's Go runtime streams for one turn
    /// (`go/adk/pkg/a2a/executor.go` `Execute`, read 2026-10-07): the submitted task, a `working`
    /// status without words, one `working` status per model event whose agent message carries the
    /// text (a streamed delta marked `adk_partial`, then the whole text), the last non-partial
    /// event's text as ONE unnamed artifact (`NewArtifactEvent`: a fresh id, no name, no append)
    /// with `lastChunk: true`, and `completed` with no message. Nothing says `text-stream/v1`.
    fn kagent_turn(answer: &str) -> Vec<StreamResponse> {
        let partial = |id: &str, text: &str| {
            let mut m = msg(id, Role::Agent, text);
            m.metadata = Some(HashMap::from([("adk_partial".to_owned(), json!(true))]));
            status_update(TaskState::Working, Some(m))
        };
        vec![
            StreamResponse::Task(task(TaskState::Submitted, vec![], None)),
            status_update(
                TaskState::Submitted,
                Some(msg("u-1", Role::User, "Say hello")),
            ),
            status_update(TaskState::Working, None),
            partial("p-1", "kagent says "),
            partial("p-2", "hello"),
            status_update(TaskState::Working, Some(msg("w-1", Role::Agent, answer))),
            artifact_update(
                art("art-1", None, vec![Part::text(answer)]),
                None,
                Some(true),
            ),
            status_update(TaskState::Completed, None),
        ]
    }

    fn all(mapper: &mut StreamMapper, items: Vec<StreamResponse>) -> Vec<AgentEnvelope> {
        items.into_iter().flat_map(|i| ok(mapper.map(i))).collect()
    }

    fn answers(envs: &[AgentEnvelope]) -> Vec<&AgentEnvelope> {
        envs.iter()
            .filter(|e| matches!(e.update, Some(AgentUpdate::Message { .. })))
            .collect()
    }

    fn artifacts_of(envs: &[AgentEnvelope]) -> usize {
        envs.iter()
            .filter(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
            .count()
    }

    #[test]
    fn kagents_unnamed_text_artifact_is_the_answer_said_once_before_the_completion() {
        let envs = all(
            &mut StreamMapper::default(),
            kagent_turn("kagent says hello"),
        );
        let said = answers(&envs);
        assert_eq!(said.len(), 1, "{envs:?}");
        assert_eq!(
            said[0].update,
            Some(AgentUpdate::Message {
                message_id: "task-1:artifact:art-1".into(),
                text: "kagent says hello".into(),
                is_final: true,
                purpose: Some(MessagePurpose::Answer),
            })
        );
        assert_eq!(
            said[0].key,
            IdemKey::Task("a2a:msg:task-1:artifact:art-1".into())
        );
        assert_eq!(said[0].task_state, None);
        assert_eq!(
            artifacts_of(&envs),
            0,
            "the text is the answer, not also an artifact"
        );
        // the answer comes first and the completion last
        let last = envs.last().unwrap();
        assert_eq!(last.task_state, Some(AgentTaskState::Completed));
        assert_eq!(envs[envs.len() - 2].update, said[0].update, "{envs:?}");
        // the working statuses are what they were: status words, never the answer
        let working_words: Vec<_> = envs
            .iter()
            .filter_map(|e| match &e.update {
                Some(AgentUpdate::Status {
                    state: AgentTaskState::Working,
                    detail: Some(d),
                }) => Some(d.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            working_words,
            vec!["kagent says ", "hello", "kagent says hello"]
        );
    }

    #[test]
    fn a_text_artifact_sent_in_appended_chunks_is_one_answer_with_the_chunks_joined_as_written() {
        let mut m = StreamMapper::default();
        let mut envs = all(
            &mut m,
            vec![
                status_update(TaskState::Working, None),
                artifact_update(art("a", None, vec![Part::text("kagent says ")]), None, None),
                artifact_update(art("a", None, vec![Part::text("hel")]), Some(true), None),
                artifact_update(
                    art("a", None, vec![Part::text("lo")]),
                    Some(true),
                    Some(true),
                ),
            ],
        );
        assert!(answers(&envs).is_empty(), "held until the turn ends");
        envs.extend(ok(m.map(status_update(TaskState::Completed, None))));
        let said = answers(&envs);
        assert_eq!(said.len(), 1, "{envs:?}");
        assert!(matches!(
            &said[0].update,
            Some(AgentUpdate::Message { text, .. }) if text == "kagent says hello"
        ));
        assert_eq!(artifacts_of(&envs), 0);
    }

    #[test]
    fn two_unnamed_text_artifacts_are_one_answer_in_order() {
        let mut m = StreamMapper::default();
        let envs = all(
            &mut m,
            vec![
                artifact_update(art("a", None, vec![Part::text("one")]), None, Some(true)),
                artifact_update(art("b", None, vec![Part::text("two")]), None, Some(true)),
                status_update(TaskState::Completed, None),
            ],
        );
        let said = answers(&envs);
        assert_eq!(said.len(), 1, "{envs:?}");
        assert!(matches!(
            &said[0].update,
            Some(AgentUpdate::Message { message_id, text, .. })
                if text == "one\n\ntwo" && message_id == "task-1:artifact:a"
        ));
    }

    #[test]
    fn a_stream_that_said_something_keeps_its_artifacts_as_artifacts() {
        // an agent that states messages (adam's text-stream, a plain A2A message) has its answer
        let mut m = StreamMapper::default();
        let mut said = msg("m-1", Role::Agent, "Here you go.");
        said.task_id = Some(T.into());
        let envs = all(
            &mut m,
            vec![
                StreamResponse::Message(said),
                artifact_update(art("a", None, vec![Part::text("notes")]), None, Some(true)),
                status_update(TaskState::Completed, None),
            ],
        );
        assert_eq!(answers(&envs).len(), 1, "only the message: {envs:?}");
        assert_eq!(artifacts_of(&envs), 1, "{envs:?}");
        let at = |f: &dyn Fn(&AgentEnvelope) -> bool| envs.iter().position(f).unwrap();
        assert!(
            at(&|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
                < at(&|e| e.task_state == Some(AgentTaskState::Completed)),
            "the artifact is still before the completion"
        );
    }

    #[test]
    fn a_completion_with_words_keeps_the_artifact_as_an_artifact() {
        let mut m = StreamMapper::default();
        let envs = all(
            &mut m,
            vec![
                artifact_update(art("a", None, vec![Part::text("notes")]), None, Some(true)),
                status_update(
                    TaskState::Completed,
                    Some(msg("s-1", Role::Agent, "All done.")),
                ),
            ],
        );
        assert!(
            answers(&envs).is_empty(),
            "the status words are the answer: {envs:?}"
        );
        assert_eq!(artifacts_of(&envs), 1);
    }

    #[test]
    fn a_named_artifact_is_a_deliverable_and_never_the_answer() {
        let mut m = StreamMapper::default();
        let envs = all(
            &mut m,
            vec![
                artifact_update(
                    art(
                        "a",
                        Some("pull_request"),
                        vec![Part::text("https://x/pull/1")],
                    ),
                    None,
                    Some(true),
                ),
                status_update(TaskState::Completed, None),
            ],
        );
        assert!(answers(&envs).is_empty(), "{envs:?}");
        assert_eq!(artifacts_of(&envs), 1);
    }

    #[test]
    fn data_files_and_links_are_never_the_answer_even_unnamed() {
        for part in [
            Part::data(json!({"passed": true})),
            Part::raw(b"bytes".to_vec()),
            Part::url("https://x/y"),
        ] {
            let mut m = StreamMapper::default();
            let envs = all(
                &mut m,
                vec![
                    artifact_update(
                        art("a", None, vec![Part::text("see this"), part]),
                        None,
                        Some(true),
                    ),
                    status_update(TaskState::Completed, None),
                ],
            );
            assert!(answers(&envs).is_empty(), "{envs:?}");
        }
    }

    #[test]
    fn a_turn_that_does_not_complete_gives_its_unnamed_text_back_as_an_artifact() {
        for state in [
            TaskState::Failed,
            TaskState::Canceled,
            TaskState::InputRequired,
            TaskState::Rejected,
        ] {
            let mut m = StreamMapper::default();
            let envs = all(
                &mut m,
                vec![
                    artifact_update(
                        art("a", None, vec![Part::text("partial")]),
                        None,
                        Some(true),
                    ),
                    status_update(state.clone(), None),
                ],
            );
            assert!(answers(&envs).is_empty(), "{state:?}: {envs:?}");
            assert_eq!(artifacts_of(&envs), 1, "{state:?}");
            assert!(
                matches!(envs[0].update, Some(AgentUpdate::Artifact { .. })),
                "the artifact comes before the status that ends the turn"
            );
        }
    }

    #[test]
    fn a_stream_that_ends_with_the_answer_held_says_nothing_and_the_poll_has_it() {
        let mut m = StreamMapper::default();
        let envs = all(
            &mut m,
            vec![
                status_update(TaskState::Working, None),
                artifact_update(
                    art("art-1", None, vec![Part::text("hello")]),
                    None,
                    Some(true),
                ),
            ],
        );
        assert!(
            answers(&envs).is_empty() && artifacts_of(&envs) == 0,
            "{envs:?}"
        );
        // the poll: the same key as the stream would have said
        let snap = snapshot(&task(
            TaskState::Completed,
            vec![art("art-1", None, vec![Part::text("hello")])],
            None,
        ))
        .unwrap();
        let said = answers(&snap.envelopes);
        assert_eq!(said.len(), 1);
        assert_eq!(
            said[0].key,
            IdemKey::Task("a2a:msg:task-1:artifact:art-1".into())
        );
        assert_eq!(artifacts_of(&snap.envelopes), 0);
    }

    #[test]
    fn a_poll_and_the_stream_say_the_same_answer_under_one_key() {
        let live = all(
            &mut StreamMapper::default(),
            kagent_turn("kagent says hello"),
        );
        let snap = snapshot(&task(
            TaskState::Completed,
            vec![art("art-1", None, vec![Part::text("kagent says hello")])],
            None,
        ))
        .unwrap();
        let (a, b) = (answers(&live), answers(&snap.envelopes));
        assert_eq!((a.len(), b.len()), (1, 1));
        assert_eq!((&a[0].key, &a[0].update), (&b[0].key, &b[0].update));
    }

    #[test]
    fn a_poll_of_a_task_that_is_not_complete_or_that_has_words_keeps_the_artifact() {
        for (state, words) in [
            (TaskState::Working, None),
            (TaskState::Failed, None),
            (TaskState::Completed, Some(msg("s", Role::Agent, "Done."))),
        ] {
            let snap = snapshot(&task(
                state.clone(),
                vec![art("a", None, vec![Part::text("t")])],
                words,
            ))
            .unwrap();
            assert!(answers(&snap.envelopes).is_empty(), "{state:?}");
            assert_eq!(artifacts_of(&snap.envelopes), 1, "{state:?}");
        }
    }

    #[test]
    fn a_blank_unnamed_text_artifact_is_not_an_answer() {
        let envs = all(
            &mut StreamMapper::default(),
            vec![
                artifact_update(art("a", None, vec![Part::text("  \n")]), None, Some(true)),
                status_update(TaskState::Completed, None),
            ],
        );
        assert!(answers(&envs).is_empty(), "{envs:?}");
    }

    // ---- usage/v1 (ADR 0056) ------------------------------------------------------------------

    fn usage_update(state: TaskState, message: Option<Message>, entry: Value) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: T.into(),
            context_id: C.into(),
            status: status(state, message),
            metadata: Some(HashMap::from([(USAGE_EXTENSION.to_owned(), entry)])),
        })
    }

    fn call_report() -> Value {
        json!({"call": "c7", "stepId": "tool:call_2", "provider": "openai", "model": "glm-5.3",
               "inputTokens": 41250, "outputTokens": 812, "totalTokens": 42062,
               "contextWindow": 131072})
    }

    fn totals_entry() -> Value {
        json!({"totals": [{"provider": "openai", "model": "glm-5.3", "inputTokens": 512000,
                           "outputTokens": 9100, "totalTokens": 521100}]})
    }

    #[test]
    fn a_working_update_with_a_report_and_no_message_is_the_report_and_nothing_else() {
        let env = only(StreamMapper::default().map(usage_update(
            TaskState::Working,
            None,
            call_report(),
        )));
        assert_eq!(env.task_state, Some(AgentTaskState::Working));
        assert_eq!(env.key, IdemKey::Task("a2a:task-1:usage:c7".into()));
        let Some(AgentUpdate::Usage(UsageUpdate::Call(call))) = &env.update else {
            panic!("{env:?}")
        };
        assert_eq!(call.task, T);
        assert_eq!(call.step.as_deref(), Some("task-1/tool:call_2"));
        assert_eq!(call.tokens.total_tokens, 42062);
        assert_eq!(call.context_window, Some(131_072));
    }

    #[test]
    fn a_report_beside_a_message_keeps_the_status_it_came_on() {
        let envs = ok(StreamMapper::default().map(usage_update(
            TaskState::Working,
            Some(msg("m1", Role::Agent, "Reading the code")),
            call_report(),
        )));
        assert_eq!(envs.len(), 2, "{envs:?}");
        assert!(matches!(envs[0].update, Some(AgentUpdate::Usage(_))));
        assert!(matches!(
            &envs[1].update,
            Some(AgentUpdate::Status { state: AgentTaskState::Working, detail: Some(d) }) if d == "Reading the code"
        ));
    }

    #[test]
    fn a_report_that_breaks_the_contract_is_rejected_and_is_no_status_either() {
        let mut bad = call_report();
        bad["totalTokens"] = json!(1);
        let env = only(StreamMapper::default().map(usage_update(TaskState::Working, None, bad)));
        assert_eq!(
            env.update,
            Some(AgentUpdate::UsageRejected(
                orch_core::UsageInvalid::TotalNotSum
            ))
        );
    }

    #[test]
    fn totals_on_a_status_that_ends_or_pauses_the_turn_come_before_it() {
        for (state, neutral) in [
            (TaskState::Completed, AgentTaskState::Completed),
            (TaskState::Failed, AgentTaskState::Failed),
            (TaskState::Canceled, AgentTaskState::Canceled),
            (TaskState::Rejected, AgentTaskState::Rejected),
            (TaskState::InputRequired, AgentTaskState::InputRequired),
            (TaskState::AuthRequired, AgentTaskState::AuthRequired),
        ] {
            let envs = ok(StreamMapper::default().map(usage_update(state, None, totals_entry())));
            assert_eq!(envs.len(), 2, "{envs:?}");
            assert_eq!(
                envs[0].key,
                IdemKey::Turn(format!("task-1:usage-total:{}", slug(neutral)))
            );
            assert_eq!(envs[0].task_state, None, "the status says the state");
            let Some(AgentUpdate::Usage(UsageUpdate::Total(totals))) = &envs[0].update else {
                panic!("{envs:?}")
            };
            assert_eq!(totals.totals[0].tokens.total_tokens, 521_100);
            assert!(matches!(
                envs[1].update,
                Some(AgentUpdate::Status { state, .. }) if state == neutral
            ));
        }
    }

    #[test]
    fn an_entry_on_another_state_or_under_a_near_miss_uri_is_a_plain_status() {
        let env = only(StreamMapper::default().map(usage_update(
            TaskState::Submitted,
            None,
            call_report(),
        )));
        assert_eq!(env.update, None);
        for near in [
            "https://agents.vymalo.com/a2a/extensions/usage/v2",
            "https://agents.vymalo.com/a2a/extensions/usage/v1/",
        ] {
            let env = only(StreamMapper::default().map(StreamResponse::StatusUpdate(
                TaskStatusUpdateEvent {
                    task_id: T.into(),
                    context_id: C.into(),
                    status: status(TaskState::Working, None),
                    metadata: Some(HashMap::from([(near.to_owned(), call_report())])),
                },
            )));
            assert!(
                matches!(env.update, Some(AgentUpdate::Status { .. })),
                "{near}"
            );
        }
    }

    #[test]
    fn a_poll_says_the_tasks_totals_under_the_keys_of_the_stream() {
        let mut finished = task(TaskState::Completed, vec![], None);
        finished.metadata = Some(HashMap::from([(
            USAGE_EXTENSION.to_owned(),
            totals_entry(),
        )]));
        let snap = snapshot(&finished).unwrap();
        let streamed = ok(StreamMapper::default().map(usage_update(
            TaskState::Completed,
            None,
            totals_entry(),
        )));
        assert_eq!(snap.envelopes.len(), 2, "{:?}", snap.envelopes);
        assert_eq!(
            (&snap.envelopes[0].key, &snap.envelopes[0].update),
            (&streamed[0].key, &streamed[0].update)
        );
        assert_eq!(
            usage_totals(&finished).map(|e| e.key),
            Some(streamed[0].key.clone())
        );
        // a task that works holds no totals yet
        let mut working = finished.clone();
        working.status = status(TaskState::Working, None);
        assert_eq!(usage_totals(&working), None);
        assert!(
            snapshot(&working)
                .unwrap()
                .envelopes
                .iter()
                .all(|e| !matches!(e.update, Some(AgentUpdate::Usage(_))))
        );
    }

    #[test]
    fn an_end_without_totals_is_what_the_adapter_reads_the_task_for() {
        assert_eq!(
            wants_usage_totals(&status_update(TaskState::Completed, None)),
            Some(T)
        );
        assert_eq!(
            wants_usage_totals(&status_update(TaskState::InputRequired, None)),
            Some(T)
        );
        assert_eq!(
            wants_usage_totals(&usage_update(TaskState::Completed, None, totals_entry())),
            None
        );
        assert_eq!(
            wants_usage_totals(&status_update(TaskState::Working, None)),
            None
        );
        assert_eq!(
            wants_usage_totals(&StreamResponse::Task(task(
                TaskState::Completed,
                vec![],
                None
            ))),
            None
        );
    }
}
