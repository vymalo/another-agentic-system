//! A person's memory (`docs/api/memory-v1.md`, ADR 0091): the types and the rules, pure.
//!
//! A note is one person's short text. This module decides, with no I/O, which memory tools a caller
//! may use ([`check_call`]), what a call's input must look like ([`ReadInput`], [`SaveInput`],
//! [`ForgetInput`]), and what a person's notes ([`Memory`]) and settings ([`MemorySettings`]) do on
//! each call. Ids and times come in as arguments; the store, the routes and the tools are later slices.
//! The vectors in `docs/api/examples/memory/` are the contract for all of it (`tests/memory_vectors.rs`).

use std::collections::BTreeMap;

use serde_json::Value;

/// The longest note, in bytes, after trimming.
pub const MAX_NOTE_BYTES: usize = 1024;
/// The most notes a person holds; a save over it is refused, never an eviction.
pub const MAX_NOTES: usize = 200;
/// The most notes one read returns.
pub const MAX_READ_NOTES: usize = 50;
/// The longest `query` of a read, in bytes.
pub const MAX_QUERY_BYTES: usize = 128;

/// A memory tool of `thread-tools/v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTool {
    /// `memory_read`.
    Read,
    /// `memory_save`.
    Save,
    /// `memory_forget`.
    Forget,
}

impl MemoryTool {
    /// The tool's name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "memory_read",
            Self::Save => "memory_save",
            Self::Forget => "memory_forget",
        }
    }

    /// The tool of a wire name, if it is one of the three.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "memory_read" => Some(Self::Read),
            "memory_save" => Some(Self::Save),
            "memory_forget" => Some(Self::Forget),
            _ => None,
        }
    }
}

/// What a person let one agent do with their memory. Both are `false` by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Grant {
    /// May read notes.
    pub read: bool,
    /// May save and forget notes.
    pub write: bool,
}

impl Grant {
    /// Whether this grant covers `tool`.
    pub fn allows(self, tool: MemoryTool) -> bool {
        match tool {
            MemoryTool::Read => self.read,
            MemoryTool::Save | MemoryTool::Forget => self.write,
        }
    }
}

/// A thread's switch; off by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThreadSwitch {
    /// No memory tool is listed.
    #[default]
    Off,
    /// The tools are listed to a main caller that holds the grant.
    On,
}

/// Why a memory call is refused, in the order the contract checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MemoryError {
    /// The thread's switch is off.
    #[error("memory_off")]
    MemoryOff,
    /// The agent lacks the grant for this tool.
    #[error("no_grant")]
    NoGrant,
    /// The caller is a child session or an asked agent.
    #[error("not_main_caller")]
    NotMainCaller,
    /// A note over [`MAX_NOTE_BYTES`]. The error names the bound and never the text.
    #[error("too_long (at most {MAX_NOTE_BYTES} bytes)")]
    TooLong,
    /// The person already holds [`MAX_NOTES`].
    #[error("too_many (at most {MAX_NOTES} notes)")]
    TooMany,
    /// A member of the input is of the wrong type, or empty, or `query` is too long.
    #[error("invalid")]
    Invalid,
}

impl MemoryError {
    /// The code on the wire.
    pub fn code(self) -> &'static str {
        match self {
            Self::MemoryOff => "memory_off",
            Self::NoGrant => "no_grant",
            Self::NotMainCaller => "not_main_caller",
            Self::TooLong => "too_long",
            Self::TooMany => "too_many",
            Self::Invalid => "invalid",
        }
    }
}

/// The answer of [`check_call`] when a call must not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The tool is not listed to this caller: answered as any other unknown name.
    UnknownTool,
    /// The tool is known and refused, with its code.
    Refused(MemoryError),
}

/// Whether `tools/list` shows `tool`: to the thread's main caller only, while the switch is on, and
/// only to an agent holding the grant for it.
pub fn listed(
    tool: MemoryTool,
    switch: ThreadSwitch,
    main_caller: bool,
    grant: Option<Grant>,
) -> bool {
    main_caller && switch == ThreadSwitch::On && grant.is_some_and(|g| g.allows(tool))
}

/// Decides whether a memory call may run. An agent with no entry in the person's settings is not
/// offered the tools (`UnknownTool`); otherwise the first that applies of `memory_off`, `no_grant`,
/// `not_main_caller` wins. A child caller with no grant at all is `UnknownTool` too: the two
/// refusals reveal nothing different.
pub fn check_call(
    tool: MemoryTool,
    switch: ThreadSwitch,
    main_caller: bool,
    grant: Option<Grant>,
) -> Result<(), Refusal> {
    let Some(grant) = grant else {
        return Err(Refusal::UnknownTool);
    };
    if switch == ThreadSwitch::Off {
        return Err(Refusal::Refused(MemoryError::MemoryOff));
    }
    if !grant.allows(tool) {
        return Err(Refusal::Refused(MemoryError::NoGrant));
    }
    if !main_caller {
        return Err(Refusal::Refused(MemoryError::NotMainCaller));
    }
    Ok(())
}

/// One saved note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// `m-` and an opaque id.
    pub id: String,
    /// The trimmed text.
    pub text: String,
    /// RFC 3339, set by the caller of [`Memory::save`].
    pub saved_at: String,
    /// The agent that saved it, or `person`.
    pub written_by: String,
}

/// The input of `memory_read`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadInput {
    /// A case-insensitive substring filter.
    pub query: Option<String>,
    /// How many notes at most, `1..=MAX_READ_NOTES`.
    pub limit: usize,
}

impl ReadInput {
    /// Parses and checks the input; the limit is clamped to `1..=MAX_READ_NOTES`.
    pub fn parse(input: &Value) -> Result<Self, MemoryError> {
        let obj = input.as_object().ok_or(MemoryError::Invalid)?;
        let query = match obj.get("query") {
            None | Some(Value::Null) => None,
            Some(Value::String(q)) if q.len() <= MAX_QUERY_BYTES => Some(q.clone()),
            Some(_) => return Err(MemoryError::Invalid),
        };
        let limit = match obj.get("limit") {
            None | Some(Value::Null) => MAX_READ_NOTES,
            Some(v) => {
                let n = v.as_i64().ok_or(MemoryError::Invalid)?;
                n.clamp(1, MAX_READ_NOTES as i64) as usize
            }
        };
        Ok(Self { query, limit })
    }
}

/// The input of `memory_save`: the trimmed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveInput(pub String);

impl SaveInput {
    /// Trims the text; empty is `Invalid`, over [`MAX_NOTE_BYTES`] is `TooLong`.
    pub fn parse(input: &Value) -> Result<Self, MemoryError> {
        let text = input
            .get("text")
            .and_then(Value::as_str)
            .ok_or(MemoryError::Invalid)?
            .trim();
        if text.is_empty() {
            return Err(MemoryError::Invalid);
        }
        if text.len() > MAX_NOTE_BYTES {
            return Err(MemoryError::TooLong);
        }
        Ok(Self(text.to_owned()))
    }
}

/// The input of `memory_forget`: a note id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetInput(pub String);

impl ForgetInput {
    /// Reads the note id; absent, empty or not text is `Invalid`.
    pub fn parse(input: &Value) -> Result<Self, MemoryError> {
        match input.get("id").and_then(Value::as_str) {
            Some(id) if !id.is_empty() => Ok(Self(id.to_owned())),
            _ => Err(MemoryError::Invalid),
        }
    }
}

/// The answer of a read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    /// Newest first, at most the limit.
    pub notes: Vec<Note>,
    /// Notes that matched, before the limit.
    pub total: usize,
    /// More matched than were returned.
    pub truncated: bool,
}

/// One person's notes, oldest first as saved. Pure: a store holds the same rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Memory {
    notes: Vec<Note>,
}

impl Memory {
    /// How many notes are held.
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// Whether no note is held.
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Stores a note at once (there is no approval step). Refused at [`MAX_NOTES`].
    pub fn save(
        &mut self,
        text: SaveInput,
        written_by: &str,
        id: String,
        saved_at: String,
    ) -> Result<Note, MemoryError> {
        if self.notes.len() >= MAX_NOTES {
            return Err(MemoryError::TooMany);
        }
        let note = Note {
            id,
            text: text.0,
            saved_at,
            written_by: written_by.to_owned(),
        };
        self.notes.push(note.clone());
        Ok(note)
    }

    /// The notes matching the query (case-insensitive substring), newest first, cut at the limit.
    pub fn read(&self, input: &ReadInput) -> ReadResult {
        let needle = input.query.as_deref().map(str::to_lowercase);
        let matched: Vec<&Note> = self
            .notes
            .iter()
            .rev()
            .filter(|n| {
                needle
                    .as_deref()
                    .is_none_or(|q| n.text.to_lowercase().contains(q))
            })
            .collect();
        let total = matched.len();
        let notes: Vec<Note> = matched
            .into_iter()
            .take(input.limit.clamp(1, MAX_READ_NOTES))
            .cloned()
            .collect();
        ReadResult {
            truncated: total > notes.len(),
            notes,
            total,
        }
    }

    /// Deletes a note; `false` for an id that is not there. Idempotent.
    pub fn forget(&mut self, id: &ForgetInput) -> bool {
        let before = self.notes.len();
        self.notes.retain(|n| n.id != id.0);
        self.notes.len() != before
    }
}

/// A grant change refused: the body is not `{agent: text, read: bool, write: bool}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid")]
pub struct InvalidSettings;

/// A person's grants: the default, and one entry per agent they touched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemorySettings {
    /// Always both `false`; no route changes it.
    pub default: Grant,
    /// The grant of each agent the person touched.
    pub agents: BTreeMap<String, Grant>,
}

impl MemorySettings {
    /// The grant of an agent the person touched.
    pub fn grant(&self, agent: &str) -> Option<Grant> {
        self.agents.get(agent).copied()
    }

    /// Applies a `PUT` body. Nothing changes when it is refused. Returns the agent whose grant it set.
    pub fn apply(&mut self, body: &Value) -> Result<String, InvalidSettings> {
        let obj = body.as_object().ok_or(InvalidSettings)?;
        let agent = obj
            .get("agent")
            .and_then(Value::as_str)
            .filter(|a| !a.is_empty())
            .ok_or(InvalidSettings)?;
        let read = obj
            .get("read")
            .and_then(Value::as_bool)
            .ok_or(InvalidSettings)?;
        let write = obj
            .get("write")
            .and_then(Value::as_bool)
            .ok_or(InvalidSettings)?;
        self.agents.insert(agent.to_owned(), Grant { read, write });
        Ok(agent.to_owned())
    }
}
