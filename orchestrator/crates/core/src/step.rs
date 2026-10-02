//! Nested steps (ADR 0025, `docs/api/steps-v1.md`).
//!
//! An agent's work has a shape: the agent, a sub-agent it delegated to, the tools and commands
//! they ran. A [`StepReport`] says one step of it is in a state; the core turns a stream of
//! reports into `agent_step` events ([`AgentStepData`]) with the **source path** of the step (the
//! chain of step ids down to it), and keeps the log bounded while it does:
//!
//! - the first report of an id *starts* the step, the last (a completed, failed or canceled one)
//!   *ends* it, and in between **at most [`MAX_STEP_UPDATES`]** updates are logged: the rest are
//!   dropped without a trace, so a sub-agent that reports every line of output cannot fill the
//!   log;
//! - a job logs at most [`MAX_STEPS_PER_JOB`] steps and tracks at most [`MAX_OPEN_STEPS`] at a
//!   time.
//!
//! What is tracked lives in the job ledger ([`StepLedger`], `Job.steps`), so any replica decides
//! the same way. [`record_step`] is a free function over the ledger on purpose: the steps an
//! agent reports ([`AgentUpdate::Step`](crate::AgentUpdate::Step)) and the steps the
//! orchestrator reports itself ([`Input::Step`](crate::Input::Step): a tool call it relays, an
//! agent it asked for) go through the same rules.
//!
//! A step may carry what the tool was called with and what it returned (ADR 0030): `input` on
//! its start and `output` on its end, **cut, redacted and budgeted** at the same door: see
//! [`StepReport::sanitize`], [`StepOutput`] and [`MAX_STEP_IO_BYTES_PER_JOB`].
//!
//! Reports are **data from an agent**: [`StepReport::sanitize`] is the door, as
//! [`check_operation_list`](crate::check_operation_list) is for A2UI. What does not pass is
//! dropped, never stored.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::event::{Actor, EventBody};
use crate::redact::{redact_text, redact_value};
use crate::thread::ThreadState;
use crate::transition::{Command, append};

/// Most updates of one step the log keeps, between its start and its end (the owner's default
/// of 2026-10-01: a step is its start, its end and up to four moments in between).
pub const MAX_STEP_UPDATES: u8 = 4;
/// Most steps a job tracks as open at once.
pub const MAX_OPEN_STEPS: usize = 256;
/// Most steps a job logs.
pub const MAX_STEPS_PER_JOB: u32 = 2000;
/// Longest chain of ancestors a step's path keeps (the nearest are kept).
pub const MAX_STEP_DEPTH: usize = 8;
/// Longest step id (and parent id), in bytes. The A2A adapter prefixes an agent's id (at most 128
/// bytes) with its task id, so a thread-unique id fits.
pub const MAX_STEP_ID_BYTES: usize = 200;
/// Longest label, in characters; a longer one is cut and ends in `…`.
pub const MAX_STEP_LABEL_CHARS: usize = 200;
/// Longest detail, in characters; a longer one is cut and ends in `…`.
pub const MAX_STEP_DETAIL_CHARS: usize = 1000;

/// Most bytes of a step's `input` the log keeps (serialized JSON). A bigger one is replaced by
/// `{"_cut": true, "bytes": n}` after its long strings were cut.
pub const STEP_INPUT_MAX_BYTES: usize = 4096;
/// Longest string inside a step's `input`, in characters; a longer one is cut and ends in `…`.
pub const STEP_INPUT_STRING_MAX_CHARS: usize = 512;
/// Most bytes of a step's `output.text` the log keeps, marker included: the head and the tail of
/// a longer text (errors are at the end).
pub const STEP_OUTPUT_MAX_BYTES: usize = 8192;
/// Most bytes of `input` and `output` all the steps of one job may log together (the owner's
/// default of 2026-10-02). Past it the members are dropped and the step says so (`ioDropped`):
/// the worst case for the log is this, not 24 MiB.
pub const MAX_STEP_IO_BYTES_PER_JOB: u32 = 2 * 1024 * 1024;
/// How deep a step's `input` may nest; what is deeper is replaced by `"…"`.
const MAX_INPUT_DEPTH: usize = 12;

/// The icons an agent may name: the vocabulary of `steps/v1`. Any other value is dropped (the
/// step is kept and shows no icon), so a client draws from a fixed set.
pub const STEP_ICONS: [&str; 14] = [
    "agent", "read", "edit", "delete", "move", "search", "execute", "think", "fetch", "web", "git",
    "test", "file", "tool",
];
/// What the icon of a step the **orchestrator** reports may start with, to name an MCP server
/// attached to the thread (`mcp-server:<id>`); the client resolves it. An agent cannot name one:
/// an icon says where a step came from, and an agent does not get to say it came from a tool
/// server.
pub const MCP_SERVER_ICON_PREFIX: &str = "mcp-server:";
/// Longest server id after [`MCP_SERVER_ICON_PREFIX`], in bytes.
const MAX_ICON_SERVER_BYTES: usize = 64;

/// What a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// An agent working for the one that reported it (OpenCode under adam-coder, an agent that
    /// was asked). Its steps nest under it.
    Subagent,
    /// A tool call.
    Tool,
    /// A command that ran.
    Command,
    /// Something the agent said or decided that is worth a line of its own.
    Message,
}

impl StepKind {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            StepKind::Subagent => "subagent",
            StepKind::Tool => "tool",
            StepKind::Command => "command",
            StepKind::Message => "message",
        }
    }

    /// The kind spelled `s`; `None` for a kind this build does not know (the adapter reads it as
    /// a [`StepKind::Tool`], as `steps/v1` says).
    pub fn parse(s: &str) -> Option<Self> {
        [
            StepKind::Subagent,
            StepKind::Tool,
            StepKind::Command,
            StepKind::Message,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// Where a step stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// Working.
    Running,
    /// Waiting for something (a permission, a person, another step) and not ended.
    Waiting,
    /// Done.
    Completed,
    /// Ended badly.
    Failed,
    /// Stopped before it finished.
    Canceled,
}

impl StepState {
    /// The step has ended: no report but a retry follows.
    pub const fn is_end(self) -> bool {
        match self {
            StepState::Completed | StepState::Failed | StepState::Canceled => true,
            StepState::Running | StepState::Waiting => false,
        }
    }

    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            StepState::Running => "running",
            StepState::Waiting => "waiting",
            StepState::Completed => "completed",
            StepState::Failed => "failed",
            StepState::Canceled => "canceled",
        }
    }

    /// The state spelled `s`.
    pub fn parse(s: &str) -> Option<Self> {
        [
            StepState::Running,
            StepState::Waiting,
            StepState::Completed,
            StepState::Failed,
            StepState::Canceled,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// Which moment of a step an `agent_step` event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepPhase {
    /// The first report of the step (or the first after it ended: a retry).
    Start,
    /// A report in between, one of at most [`MAX_STEP_UPDATES`].
    Update,
    /// The report that ended it. A step that is reported ended without ever having started is one
    /// event, an `End`.
    End,
}

/// What a step returned (ADR 0030): text, cut to [`STEP_OUTPUT_MAX_BYTES`], or the error the tool
/// gave. Plain text; a screen draws it as text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepOutput {
    /// The text, with control characters but line breaks and tabs taken out, credentials
    /// redacted, and cut to [`STEP_OUTPUT_MAX_BYTES`] keeping its head and its tail.
    pub text: String,
    /// The text is not all of what the tool returned (the agent cut it, or the core did).
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// The size of what the tool returned, in bytes, when `truncated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// `text` is the error the tool returned (the step failed).
    #[serde(default, skip_serializing_if = "is_false")]
    pub error: bool,
}

/// What an agent (or the orchestrator) reported about one step: protocol-neutral, and not yet
/// trusted. `id` and `parent` are unique within the thread (the adapter makes them so).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepReport {
    /// The step's id.
    pub id: String,
    /// The step it runs under, when it runs under one reported earlier.
    pub parent: Option<String>,
    /// What it is.
    pub kind: StepKind,
    /// One line that says what it is doing.
    pub label: String,
    /// Where it stands.
    pub state: StepState,
    /// A name from [`STEP_ICONS`] (the orchestrator's own steps may also name an MCP server).
    pub icon: Option<String>,
    /// More words: the failure, the result.
    pub detail: Option<String>,
    /// What the tool was called with: a JSON object. Cut, redacted and budgeted by
    /// [`StepReport::sanitize`] and [`record_step`]; a value that is not an object is no input.
    pub input: Option<Map<String, Value>>,
    /// What the tool returned, or the error it gave. Same door.
    pub output: Option<StepOutput>,
}

/// Who a report comes from, which decides what it may name as its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepSource {
    /// An agent's `steps/v1` report: the icon is one of [`STEP_ICONS`] or none.
    Agent,
    /// The orchestrator's own: also `mcp-server:<id>`.
    Orchestrator,
}

/// `data` of an `agent_step`: one report, as the log keeps it. The last of `path` is the step's
/// parent; the first is the top of the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStepData {
    /// The step's id, unique within the thread.
    pub id: String,
    /// The ids of the steps it runs under, outermost first; empty at the top. At most
    /// [`MAX_STEP_DEPTH`].
    #[serde(default)]
    pub path: Vec<String>,
    /// What it is.
    pub kind: StepKind,
    /// One line that says what it is doing.
    pub label: String,
    /// Where it stands.
    pub state: StepState,
    /// Which moment of the step this event is.
    pub phase: StepPhase,
    /// A name from the icon vocabulary, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// More words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What the tool was called with (a JSON object of at most [`STEP_INPUT_MAX_BYTES`], or
    /// `{"_cut": true, "bytes": n}`), credentials redacted. Logged once per step, with its start
    /// (or with the first report that has it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Map<String, Value>>,
    /// What the tool returned, or the error it gave, on the step's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<StepOutput>,
    /// The step had an `input` or an `output` that the job's budget
    /// ([`MAX_STEP_IO_BYTES_PER_JOB`]) had no room for: it was dropped.
    #[serde(default, skip_serializing_if = "is_false")]
    pub io_dropped: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_icon(icon: &str, source: StepSource) -> bool {
    if STEP_ICONS.contains(&icon) {
        return true;
    }
    match source {
        StepSource::Agent => false,
        StepSource::Orchestrator => {
            icon.strip_prefix(MCP_SERVER_ICON_PREFIX)
                .is_some_and(|server| {
                    !server.is_empty()
                        && server.len() <= MAX_ICON_SERVER_BYTES
                        && server
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                })
        }
    }
}

/// A usable id: not empty, within [`MAX_STEP_ID_BYTES`], with no control character (an id reaches
/// the log, activity ids and logs).
fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_STEP_ID_BYTES && !id.chars().any(char::is_control)
}

/// `s` cut to `max` characters, ending in `…` when it was cut.
fn cut_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

impl StepReport {
    /// The report as the log may keep it, or `None` when nothing of it can be kept.
    ///
    /// - an `id` that is empty, longer than [`MAX_STEP_ID_BYTES`] or has a control character drops
    ///   the report; so does a label that is empty once its whitespace is gone;
    /// - a `parent` that is not a usable id, or is the step itself, is no parent;
    /// - a label is one line: every control character (a newline too) becomes a space, runs of
    ///   whitespace collapse, and it is cut at [`MAX_STEP_LABEL_CHARS`] with `…`;
    /// - a detail keeps its line breaks, loses other control characters, is cut at
    ///   [`MAX_STEP_DETAIL_CHARS`] with `…`, and is none when empty;
    /// - an icon outside the vocabulary for `source` is dropped, the step stays.
    ///
    /// A control character never reaches the log (Postgres refuses a NUL in JSON text).
    #[must_use]
    pub fn sanitize(&self, source: StepSource) -> Option<StepReport> {
        if !is_id(&self.id) {
            return None;
        }
        let label = self
            .label
            .split(|c: char| c.is_control() || c.is_whitespace())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if label.is_empty() {
            return None;
        }
        let parent = self
            .parent
            .as_ref()
            .filter(|p| is_id(p) && **p != self.id)
            .cloned();
        let icon = self
            .icon
            .as_deref()
            .filter(|icon| is_icon(icon, source))
            .map(str::to_owned);
        let detail = self.detail.as_deref().and_then(|d| {
            let clean: String = d
                .chars()
                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                .collect();
            let clean = clean.trim();
            (!clean.is_empty()).then(|| cut_chars(clean, MAX_STEP_DETAIL_CHARS))
        });
        Some(StepReport {
            id: self.id.clone(),
            parent,
            kind: self.kind,
            label: cut_chars(&label, MAX_STEP_LABEL_CHARS),
            state: self.state,
            icon,
            detail,
            input: self.input.as_ref().and_then(sanitize_input),
            output: self.output.as_ref().and_then(sanitize_output),
        })
    }
}

/// `s` without control characters (line breaks and tabs stay).
fn strip_controls(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

/// A JSON value with its control characters taken out of every string and key, and what nests
/// deeper than [`MAX_INPUT_DEPTH`] replaced by `"…"`.
fn clean_value(value: &Value, depth: usize) -> Value {
    match value {
        Value::String(s) => Value::String(strip_controls(s)),
        Value::Array(items) if depth >= MAX_INPUT_DEPTH && !items.is_empty() => {
            Value::String("\u{2026}".to_owned())
        }
        Value::Object(map) if depth >= MAX_INPUT_DEPTH && !map.is_empty() => {
            Value::String("\u{2026}".to_owned())
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| clean_value(v, depth + 1)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (strip_controls(k), clean_value(v, depth + 1)))
                .collect(),
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

/// Every string of `value` cut to [`STEP_INPUT_STRING_MAX_CHARS`] characters.
fn cut_strings(value: &mut Value) {
    match value {
        Value::String(s) => {
            if s.chars().count() > STEP_INPUT_STRING_MAX_CHARS {
                *s = cut_chars(s, STEP_INPUT_STRING_MAX_CHARS);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(cut_strings),
        Value::Object(map) => map.values_mut().for_each(cut_strings),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn json_len(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |s| s.len())
}

/// A step's input as the log may keep it: control characters taken out, credentials redacted
/// (the values of keys that name one, and the shapes of [`redact_text`]), strings cut to
/// [`STEP_INPUT_STRING_MAX_CHARS`], and, when it is still bigger than [`STEP_INPUT_MAX_BYTES`]
/// serialized, replaced by `{"_cut": true, "bytes": n}` with the size it had before the strings
/// were cut. An empty object is no input.
fn sanitize_input(input: &Map<String, Value>) -> Option<Map<String, Value>> {
    if input.is_empty() {
        return None;
    }
    let mut value = clean_value(&Value::Object(input.clone()), 0);
    redact_value(&mut value);
    let whole = json_len(&value);
    cut_strings(&mut value);
    let Value::Object(map) = value else {
        return None;
    };
    if json_len(&Value::Object(map.clone())) <= STEP_INPUT_MAX_BYTES {
        return Some(map);
    }
    let mut cut = Map::new();
    cut.insert("_cut".to_owned(), Value::Bool(true));
    cut.insert("bytes".to_owned(), Value::from(whole as u64));
    Some(cut)
}

/// `text` cut to `max` bytes, keeping its head (three quarters) and its tail (the rest), with a
/// line between that says how many bytes are not kept. Cuts only between characters. `text` is
/// returned as it is when it fits.
fn cut_head_tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    // the marker's length is at most that of the one that names the whole text
    let room = max.saturating_sub(marker(text.len()).len());
    let head_len = room - room / 4;
    let tail_len = room / 4;
    let mut head_end = head_len.min(text.len());
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len().saturating_sub(tail_len).max(head_end);
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}{}{}",
        &text[..head_end],
        marker(tail_start - head_end),
        &text[tail_start..]
    )
}

fn marker(omitted: usize) -> String {
    format!("\n\u{2026} {omitted} bytes not kept \u{2026}\n")
}

/// A step's output as the log may keep it: control characters taken out (line breaks and tabs
/// stay), credentials redacted, the text cut to [`STEP_OUTPUT_MAX_BYTES`] with its head and its
/// tail, `truncated` and `bytes` set when the text is not all of what was returned (an agent
/// that cut it says so itself; the size it names is kept when it is at least the text's). An
/// output with no text and no error is none.
fn sanitize_output(output: &StepOutput) -> Option<StepOutput> {
    let received = output.text.len();
    let clean = strip_controls(&output.text);
    let redacted = redact_text(&clean);
    let cut_here = redacted.len() > STEP_OUTPUT_MAX_BYTES;
    let text = cut_head_tail(&redacted, STEP_OUTPUT_MAX_BYTES);
    if text.is_empty() && !output.error {
        return None;
    }
    let truncated = output.truncated || cut_here;
    let bytes = if !truncated {
        None
    } else if cut_here {
        Some((received as u64).max(output.bytes.unwrap_or(0)))
    } else {
        output.bytes.filter(|b| *b >= text.len() as u64)
    };
    Some(StepOutput {
        text,
        truncated,
        bytes,
        error: output.error,
    })
}

/// A step the job tracks as open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OpenStep {
    /// Its path, as logged with its start.
    path: Vec<String>,
    /// How many updates were logged since it started.
    updates: u8,
    /// Its input was logged (with the start, or with the first report that had one): a later
    /// report does not log it again.
    #[serde(default, skip_serializing_if = "is_false")]
    input: bool,
}

/// The job's memory of its steps (`Job.steps`): which are open, and how many were logged. Belongs
/// to the job: [`Job::next`](crate::Job::next) forgets it. A ledger stored before the field
/// existed has none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StepLedger {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    open: BTreeMap<String, OpenStep>,
    #[serde(skip_serializing_if = "is_zero")]
    started: u32,
    /// Bytes of `input` and `output` the job's steps logged so far, against
    /// [`MAX_STEP_IO_BYTES_PER_JOB`].
    #[serde(skip_serializing_if = "is_zero")]
    io_bytes: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl StepLedger {
    /// A ledger that has seen no step, which is what a job without steps has (and what the log
    /// leaves out).
    pub fn is_empty(&self) -> bool {
        self.open.is_empty() && self.started == 0 && self.io_bytes == 0
    }

    /// How many steps are open.
    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// Whether the step `id` is open.
    pub fn is_open(&self, id: &str) -> bool {
        self.open.contains_key(id)
    }

    /// The ids of the open steps, in id order.
    pub fn open_ids(&self) -> impl Iterator<Item = &str> {
        self.open.keys().map(String::as_str)
    }

    /// How many steps this job logged.
    pub fn started(&self) -> u32 {
        self.started
    }

    /// How many bytes of step input and output this job logged, of [`MAX_STEP_IO_BYTES_PER_JOB`].
    pub fn io_bytes(&self) -> u32 {
        self.io_bytes
    }

    /// Takes what the job's budget has room for of a step's input and output, in that order.
    /// `input_wanted` is false when the step's input was logged already; the output is only
    /// wanted on an end. Returns what to log and whether something was dropped for lack of room.
    fn admit_io(
        &mut self,
        input: Option<Map<String, Value>>,
        output: Option<StepOutput>,
        input_wanted: bool,
        output_wanted: bool,
    ) -> (Option<Map<String, Value>>, Option<StepOutput>, bool) {
        let mut dropped = false;
        let mut input = input.filter(|_| input_wanted);
        if let Some(map) = &input
            && !self.take_io(json_len(&Value::Object(map.clone())))
        {
            input = None;
            dropped = true;
        }
        let mut output = output.filter(|_| output_wanted);
        if let Some(out) = &output
            && !self.take_io(out.text.len())
        {
            output = None;
            dropped = true;
        }
        (input, output, dropped)
    }

    /// Spends `size` bytes of the job's input/output budget, if it has them.
    fn take_io(&mut self, size: usize) -> bool {
        let Ok(size) = u32::try_from(size) else {
            return false;
        };
        match self.io_bytes.checked_add(size) {
            Some(total) if total <= MAX_STEP_IO_BYTES_PER_JOB => {
                self.io_bytes = total;
                true
            }
            _ => false,
        }
    }

    /// The task ended: nothing stays open. The log gets no event for it (the projection closes
    /// what it shows).
    pub(crate) fn close_all(&mut self) {
        self.open.clear();
    }

    /// The path of a step that runs under `parent`: the parent's own path and the parent, or just
    /// the parent when it is not open; at most the last [`MAX_STEP_DEPTH`] ids. A path that would
    /// hold the step itself (the parent chain loops back) is none.
    fn path_under(&self, id: &str, parent: Option<&str>) -> Vec<String> {
        let Some(parent) = parent else {
            return Vec::new();
        };
        let mut path = self
            .open
            .get(parent)
            .map(|p| p.path.clone())
            .unwrap_or_default();
        path.push(parent.to_owned());
        if path.iter().any(|p| p == id) {
            return Vec::new();
        }
        let extra = path.len().saturating_sub(MAX_STEP_DEPTH);
        path.drain(..extra);
        path
    }
}

/// What the report decides, before the state gate moves the thread.
enum Decision {
    Drop,
    Log { phase: StepPhase, path: Vec<String> },
}

fn decide(ledger: &mut StepLedger, report: &StepReport) -> Decision {
    let end = report.state.is_end();
    match ledger.open.get_mut(&report.id) {
        None => {
            if ledger.started >= MAX_STEPS_PER_JOB {
                return Decision::Drop;
            }
            if !end && ledger.open.len() >= MAX_OPEN_STEPS {
                // A step that cannot be tracked would start again with every report.
                return Decision::Drop;
            }
            let path = ledger.path_under(&report.id, report.parent.as_deref());
            ledger.started += 1;
            if end {
                return Decision::Log {
                    phase: StepPhase::End,
                    path,
                };
            }
            ledger.open.insert(
                report.id.clone(),
                OpenStep {
                    path: path.clone(),
                    updates: 0,
                    input: false,
                },
            );
            Decision::Log {
                phase: StepPhase::Start,
                path,
            }
        }
        Some(open) if end => {
            let path = open.path.clone();
            ledger.open.remove(&report.id);
            Decision::Log {
                phase: StepPhase::End,
                path,
            }
        }
        Some(open) => {
            if open.updates >= MAX_STEP_UPDATES {
                return Decision::Drop;
            }
            open.updates += 1;
            Decision::Log {
                phase: StepPhase::Update,
                path: open.path.clone(),
            }
        }
    }
}

/// Records one step report: the events it gives (none, when it is dropped or coalesced) and the
/// state the thread is in after it.
///
/// | id open? | report | result |
/// |---|---|---|
/// | no | running or waiting | a `start`; tracked (dropped when `MAX_OPEN_STEPS` are open) |
/// | no | an end | one `end` event, a step of one moment |
/// | yes | running or waiting | an `update` while fewer than [`MAX_STEP_UPDATES`] were logged, else nothing |
/// | yes | an end | an `end` with the path the step started with; no longer open |
///
/// A job past [`MAX_STEPS_PER_JOB`] starts no more steps.
///
/// The state gate: a `queued` thread whose agent reports a step is `working` (no event of its
/// own; the step is the sign), a `working` thread stays so, and in `blocked` or `verifying` the
/// work is not going on, so a late report is dropped. A finished thread takes none: callers
/// refuse those first (`TransitionError::InvalidInState`), and here they change nothing.
///
/// A report that does not pass [`StepReport::sanitize`] changes nothing.
pub fn record_step(
    state: ThreadState,
    job: &mut crate::gate::Job,
    actor: Actor,
    report: &StepReport,
    source: StepSource,
) -> (ThreadState, Vec<Command>) {
    match state {
        ThreadState::Queued | ThreadState::Working => {}
        ThreadState::Blocked
        | ThreadState::Verifying
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => return (state, Vec::new()),
    }
    let Some(report) = report.sanitize(source) else {
        return (state, Vec::new());
    };
    let input_logged = job.steps.open.get(&report.id).is_some_and(|o| o.input);
    let Decision::Log { phase, path } = decide(&mut job.steps, &report) else {
        return (state, Vec::new());
    };
    // the input once per step, the output on its end, both within the job's budget
    let (input, output, io_dropped) = job.steps.admit_io(
        report.input,
        report.output,
        !input_logged,
        phase == StepPhase::End,
    );
    if input.is_some()
        && let Some(open) = job.steps.open.get_mut(&report.id)
    {
        open.input = true;
    }
    let event = append(
        actor,
        EventBody::AgentStep(AgentStepData {
            id: report.id,
            path,
            kind: report.kind,
            label: report.label,
            state: report.state,
            phase,
            icon: report.icon,
            detail: report.detail,
            input,
            output,
            io_dropped,
        }),
    );
    (ThreadState::Working, vec![event])
}
