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
    A2UI_MEDIA_TYPE, AgentTaskState, AgentUpdate, LiveChunk, LiveEnd, MessagePurpose,
    STEPS_EXTENSION, StepKind, StepOutput, StepReport, StepState, TEXT_STREAM_EXTENSION,
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

/// The chunk an artifact update is, under `text-stream/v1`; `None` for an update that is not
/// one, or whose entry does not validate (it is then a plain artifact chunk).
fn live_chunk_of(update: &TaskArtifactUpdateEvent) -> Option<LiveChunk> {
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
    // Giving up is said on the last chunk; said without `lastChunk` it ends the stream too.
    let abandoned = entry.get("abandoned").and_then(Value::as_bool) == Some(true);
    let end = if abandoned {
        LiveEnd::Abandoned
    } else if update.last_chunk == Some(true) {
        LiveEnd::Last
    } else {
        LiveEnd::Open
    };
    Some(LiveChunk {
        message_id: artifact.artifact_id.clone(),
        offset,
        text: text.clone(),
        end,
    })
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

/// Every artifact of a task, merging entries that share an id (the server appends chunk by
/// chunk without merging), then the status.
fn task_envelopes(task: &Task) -> Vec<AgentEnvelope> {
    let revision = revision_of(&task.metadata);
    let mut order: Vec<&str> = Vec::new();
    let mut merged: HashMap<&str, (Option<&str>, Vec<Part>)> = HashMap::new();
    // A chunk of streamed text is transient: a task that holds one (an agent that kept it) is not
    // showing an artifact.
    for a in task
        .artifacts
        .iter()
        .flatten()
        .filter(|a| text_stream_entry(a).is_none())
    {
        let entry = merged.entry(a.artifact_id.as_str()).or_insert_with(|| {
            order.push(a.artifact_id.as_str());
            (a.name.as_deref(), Vec::new())
        });
        entry.1.extend(a.parts.iter().cloned());
    }
    let mut out: Vec<AgentEnvelope> = order
        .into_iter()
        .filter_map(|id| {
            let (name, parts) = merged.get(id)?;
            Some(artifact_envelopes(
                &task.id,
                &task.context_id,
                id,
                *name,
                parts,
                revision.clone(),
            ))
        })
        .flatten()
        .collect();
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

/// Stream-local state: the task the stream belongs to, and the artifact held back (if any).
#[derive(Default)]
pub struct StreamMapper {
    task_id: Option<String>,
    context_id: Option<String>,
    pending: Option<Pending>,
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
            if let Some(chunk) = live_chunk_of(&u) {
                // Not an artifact, and an event like any other for one that was held back.
                let mut out: Vec<Result<AgentEnvelope, AgentError>> = self
                    .pending
                    .take()
                    .map(Pending::into_envelopes)
                    .into_iter()
                    .flatten()
                    .map(Ok)
                    .collect();
                out.push(Ok(live_envelope(&u, chunk)));
                return out;
            }
            return self.artifact_update(u).into_iter().map(Ok).collect();
        }
        // Any other event ends a held-back artifact: nothing more is appended to it.
        let mut out: Vec<Result<AgentEnvelope, AgentError>> = self
            .pending
            .take()
            .map(Pending::into_envelopes)
            .into_iter()
            .flatten()
            .map(Ok)
            .collect();
        match item {
            StreamResponse::Task(task) => {
                self.learn(&task.id, &task.context_id);
                out.extend(task_envelopes(&task).into_iter().map(Ok));
            }
            StreamResponse::StatusUpdate(u) => {
                self.learn(&u.task_id, &u.context_id);
                out.extend(
                    status_envelopes(
                        &u.task_id,
                        &u.context_id,
                        &u.status,
                        revision_of(&u.metadata),
                    )
                    .into_iter()
                    .map(Ok),
                );
            }
            StreamResponse::Message(m) => out.extend(self.message(&m)),
            StreamResponse::ArtifactUpdate(_) => {}
        }
        out
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
                p.parts.extend(u.artifact.parts);
                p.revision = revision.or(p.revision.take());
            }
            p
        } else {
            // A different artifact, or the same id sent whole again (which replaces it).
            if !same_artifact {
                out.extend(
                    self.pending
                        .take()
                        .into_iter()
                        .flat_map(Pending::into_envelopes),
                );
            } else {
                self.pending = None;
            }
            Some(Pending {
                task_id: u.task_id,
                context_id: u.context_id,
                artifact_id: u.artifact.artifact_id,
                name: u.artifact.name,
                parts: u.artifact.parts,
                revision,
            })
        };
        match held {
            Some(p) if last => out.extend(p.into_envelopes()),
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
                end: LiveEnd::Open
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
}
