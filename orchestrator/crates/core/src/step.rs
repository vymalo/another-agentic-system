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
//! Reports are **data from an agent**: [`StepReport::sanitize`] is the door, as
//! [`check_operation_list`](crate::check_operation_list) is for A2UI. What does not pass is
//! dropped, never stored.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::event::{Actor, EventBody};
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
        })
    }
}

/// A step the job tracks as open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OpenStep {
    /// Its path, as logged with its start.
    path: Vec<String>,
    /// How many updates were logged since it started.
    updates: u8,
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
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl StepLedger {
    /// A ledger that has seen no step, which is what a job without steps has (and what the log
    /// leaves out).
    pub fn is_empty(&self) -> bool {
        self.open.is_empty() && self.started == 0
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
    let Decision::Log { phase, path } = decide(&mut job.steps, &report) else {
        return (state, Vec::new());
    };
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
        }),
    );
    (ThreadState::Working, vec![event])
}
