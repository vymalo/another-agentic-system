//! The carry of a page of history (`docs/api/history.md`, "Carry"): what the log before the page
//! contributes to the readouts of the web that cover the whole thread, so that a page which holds
//! the newest turns still shows the thread's token totals, its files and the number of its turns.
//!
//! It is read off the frames the projector writes, not off the events: the web folds those same
//! frames (`web/src/features/chat/lib/usage.ts`), and the invariant is that the web's state built
//! from the carry of a page and the page's frames equals its state built from the whole log. The
//! pass records what it has seen at each chain start ([`Prefix`]); the carry of a page is the fold
//! of the records before its first chain.

use std::collections::{BTreeMap, HashMap, HashSet};

use orch_agui_proto as agui;
use serde_json::{Map, Value, json};

use crate::frame::Frame;
use crate::vocab::{
    ACTIVITY_A2UI_SURFACE, ACTIVITY_ACTION, ACTIVITY_ARTIFACT, ACTIVITY_ASK, ACTIVITY_CHECK,
    ACTIVITY_CI, ACTIVITY_ERROR, ACTIVITY_REWORK, ACTIVITY_STATUS, ACTIVITY_STEP, CUSTOM_USAGE,
    CUSTOM_USAGE_TOTAL,
};

/// The most kept files a carry names: the newest ones.
pub(crate) const MAX_FILES: usize = 500;

/// The largest count a page of the web reads (`Number.MAX_SAFE_INTEGER`): a larger one is no count.
const MAX_COUNT: u64 = (1 << 53) - 1;

/// How far the pass had got when a chain started.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Prefix {
    usage: usize,
    files: usize,
    turns: u64,
}

/// Everything the carry of any page of this read is made from, in the order the log said it.
#[derive(Default)]
pub(crate) struct CarryLog {
    usage: Vec<UsageRecord>,
    /// The `content` of each kept-file artifact, the first time a hash is said.
    files: Vec<Map<String, Value>>,
    file_hashes: HashSet<String>,
    /// Runs that ended with something an agent said.
    turns: u64,
    /// The open run has said something an agent says.
    output: bool,
}

impl CarryLog {
    /// Where the pass stands, for the chain that starts now.
    pub(crate) fn prefix(&self) -> Prefix {
        Prefix {
            usage: self.usage.len(),
            files: self.files.len(),
            turns: self.turns,
        }
    }

    /// Takes in the frames of one event.
    pub(crate) fn observe(&mut self, frames: &[Frame]) {
        for frame in frames {
            match &frame.event {
                agui::Event::RunStarted(_) => self.output = false,
                agui::Event::RunFinished(_) | agui::Event::RunError(_) => self.end_run(),
                agui::Event::TextMessageStart(e) => {
                    if e.role != Some(agui::TextMessageRole::User) {
                        self.output = true;
                    }
                }
                agui::Event::ReasoningStart(_) | agui::Event::ReasoningMessageStart(_) => {
                    self.output = true;
                }
                agui::Event::ActivitySnapshot(e) => {
                    self.output |= draws(&e.activity_type, &e.content);
                    if e.activity_type == ACTIVITY_ARTIFACT {
                        self.keep_file(&e.content);
                    }
                }
                agui::Event::Custom(e) if e.name == CUSTOM_USAGE => {
                    if let Some(call) = parse_call(&e.value) {
                        self.usage.push(UsageRecord::Call(call));
                    }
                }
                agui::Event::Custom(e) if e.name == CUSTOM_USAGE_TOTAL => {
                    if let Some((task, entries)) = parse_total(&e.value) {
                        self.usage.push(UsageRecord::Total { task, entries });
                    }
                }
                _ => {}
            }
        }
    }

    /// A run that said something is a turn of the agent.
    fn end_run(&mut self) {
        if std::mem::take(&mut self.output) {
            self.turns += 1;
        }
    }

    /// A file the artifact store kept (`kind: file` with its hash, route and size), once per hash.
    fn keep_file(&mut self, content: &Map<String, Value>) {
        let is_file = content.get("kind").and_then(Value::as_str) == Some("file");
        let hash = content.get("sha256").and_then(Value::as_str);
        let kept = content.get("href").is_some() && content.get("size").is_some();
        if let (true, true, Some(hash)) = (is_file, kept, hash)
            && self.file_hashes.insert(hash.to_owned())
        {
            self.files.push(content.clone());
        }
    }

    /// The carry for a page that begins where `prefix` was taken.
    pub(crate) fn carry(&self, prefix: Prefix) -> Carry {
        let files = &self.files[..prefix.files.min(self.files.len())];
        let from = files.len().saturating_sub(MAX_FILES);
        Carry {
            turns: prefix.turns,
            usage: usage_carry(&self.usage[..prefix.usage.min(self.usage.len())]),
            files: files[from..].to_vec(),
        }
    }
}

/// Whether an activity is something a turn of the agent draws: the web's `drawsPart` for the activities
/// (`web/src/features/chat/lib/steps.ts`). A status that only comes with the agent's words (`completed`,
/// `input_required`), the job marker, a fork's divider and a tools card draw nothing of their own.
fn draws(activity_type: &str, content: &Map<String, Value>) -> bool {
    match activity_type {
        ACTIVITY_STATUS => content
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| s != "completed" && s != "input_required"),
        ACTIVITY_ARTIFACT
        | ACTIVITY_STEP
        | ACTIVITY_ASK
        | ACTIVITY_CHECK
        | ACTIVITY_CI
        | ACTIVITY_REWORK
        | ACTIVITY_ACTION
        | ACTIVITY_ERROR
        | ACTIVITY_A2UI_SURFACE => true,
        _ => false,
    }
}

/// What the log before a page contributes to the readouts that cover the whole thread.
#[derive(Debug, Clone, PartialEq)]
pub struct Carry {
    /// Agent turns before the page: the runs that drew something of the agent's (words, a step, a
    /// card, a surface, a failure). The web adds it to the position of the turns it holds, so the
    /// numbers do not move when older pages load.
    pub turns: u64,
    /// The thread's token usage before the page, absent when there was none.
    pub usage: Option<Value>,
    /// The kept files before the page (the `content` of their artifacts), the newest 500, in the
    /// order they were handed over.
    pub files: Vec<Map<String, Value>>,
}

impl Carry {
    /// The carry as the contract's `HistoryCarry` (`docs/api/history.md`): `usage` and `files` are
    /// left out when empty.
    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        out.insert("turns".into(), self.turns.into());
        if let Some(usage) = &self.usage {
            out.insert("usage".into(), usage.clone());
        }
        if !self.files.is_empty() {
            let files = self.files.iter().cloned().map(Value::Object).collect();
            out.insert("files".into(), Value::Array(files));
        }
        Value::Object(out)
    }
}

// ---- usage ----------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    input: u64,
    output: u64,
    total: u64,
    reasoning: Option<u64>,
    cached: Option<u64>,
    cache_write: Option<u64>,
}

fn add_part(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    }
}

impl Counts {
    fn plus(self, o: Counts) -> Counts {
        Counts {
            input: self.input.saturating_add(o.input),
            output: self.output.saturating_add(o.output),
            total: self.total.saturating_add(o.total),
            reasoning: add_part(self.reasoning, o.reasoning),
            cached: add_part(self.cached, o.cached),
            cache_write: add_part(self.cache_write, o.cache_write),
        }
    }

    /// The counts as the members of a frame's value.
    fn put(&self, out: &mut Map<String, Value>) {
        out.insert("inputTokens".into(), self.input.into());
        out.insert("outputTokens".into(), self.output.into());
        out.insert("totalTokens".into(), self.total.into());
        for (name, part) in [
            ("reasoningTokens", self.reasoning),
            ("cachedInputTokens", self.cached),
            ("cacheWriteInputTokens", self.cache_write),
        ] {
            if let Some(n) = part {
                out.insert(name.into(), n.into());
            }
        }
    }
}

/// A provider and a model.
type ModelKey = (Option<String>, String);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    key: ModelKey,
    counts: Counts,
}

#[derive(Debug, Clone)]
struct Call {
    task: String,
    call: String,
    entry: Entry,
    context_window: Option<u64>,
    by: (String, String),
}

enum UsageRecord {
    Call(Call),
    Total { task: String, entries: Vec<Entry> },
}

fn label(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn count(v: Option<&Value>) -> Option<u64> {
    v.and_then(Value::as_u64).filter(|n| *n <= MAX_COUNT)
}

fn counts_of(v: &Map<String, Value>) -> Option<Counts> {
    Some(Counts {
        input: count(v.get("inputTokens"))?,
        output: count(v.get("outputTokens"))?,
        total: count(v.get("totalTokens"))?,
        reasoning: count(v.get("reasoningTokens")),
        cached: count(v.get("cachedInputTokens")),
        cache_write: count(v.get("cacheWriteInputTokens")),
    })
}

fn entry_of(v: &Map<String, Value>) -> Option<Entry> {
    let model = label(v.get("model"))?.to_owned();
    let provider = label(v.get("provider")).map(str::to_owned);
    Some(Entry {
        key: (provider, model),
        counts: counts_of(v)?,
    })
}

/// A `vymalo.usage` value as the web reads it (`parseCall`).
fn parse_call(value: &Value) -> Option<Call> {
    let v = value.as_object()?;
    let task = label(v.get("task"))?.to_owned();
    let call = label(v.get("call"))?.to_owned();
    let agent = label(v.get("agent")).unwrap_or("");
    let entry = entry_of(v)?;
    let by = match v.get("by").and_then(Value::as_object) {
        Some(by) => match label(by.get("kind")) {
            Some(kind @ ("agent" | "subagent" | "ask")) => (
                kind.to_owned(),
                label(by.get("name")).unwrap_or(agent).to_owned(),
            ),
            _ => ("agent".to_owned(), agent.to_owned()),
        },
        None => ("agent".to_owned(), agent.to_owned()),
    };
    Some(Call {
        task,
        call,
        entry,
        context_window: count(v.get("contextWindow")).filter(|n| *n > 0),
        by,
    })
}

/// A `vymalo.usage_total` value as the web reads it (`parseTotal`).
fn parse_total(value: &Value) -> Option<(String, Vec<Entry>)> {
    let v = value.as_object()?;
    let task = label(v.get("task"))?.to_owned();
    let totals = v.get("totals")?.as_array()?;
    let entries = totals
        .iter()
        .filter_map(|t| t.as_object().and_then(entry_of))
        .collect();
    Some((task, entries))
}

/// A task's usage as the web's fold knows it: its latest totals and the calls reported since.
#[derive(Default)]
struct Task {
    totals: Option<Vec<Entry>>,
    since: Vec<Entry>,
}

#[derive(Default)]
struct Group {
    calls: u64,
    counts: Counts,
}

/// The fold of `usage.ts` over the records, then its state as the carry says it.
fn usage_carry(records: &[UsageRecord]) -> Option<Value> {
    if records.is_empty() {
        return None;
    }
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    let mut tasks: BTreeMap<&str, Task> = BTreeMap::new();
    let mut groups: Vec<((&str, &str), Group)> = Vec::new();
    let mut place: HashMap<(&str, &str), usize> = HashMap::new();
    let mut latest: Option<&Call> = None;
    for record in records {
        match record {
            UsageRecord::Call(call) => {
                // a call is said once per task and id
                if !seen.insert((call.task.as_str(), call.call.as_str())) {
                    continue;
                }
                tasks
                    .entry(call.task.as_str())
                    .or_default()
                    .since
                    .push(call.entry.clone());
                let key = (call.by.0.as_str(), call.by.1.as_str());
                let at = *place.entry(key).or_insert_with(|| {
                    groups.push((key, Group::default()));
                    groups.len() - 1
                });
                let group = &mut groups[at].1;
                group.calls += 1;
                group.counts = group.counts.plus(call.entry.counts);
                if call.by.0 == "agent" {
                    latest = Some(call);
                }
            }
            UsageRecord::Total { task, entries } => {
                let known = tasks.entry(task.as_str()).or_default();
                // the same totals again change nothing: the calls since are still counted
                if known.totals.as_ref() == Some(entries) {
                    continue;
                }
                known.totals = Some(entries.clone());
                known.since.clear();
            }
        }
    }
    let tasks: Vec<Value> = tasks
        .into_iter()
        .map(|(task, t)| {
            let mut models: BTreeMap<&ModelKey, Counts> = BTreeMap::new();
            for e in t.totals.iter().flatten().chain(&t.since) {
                let sum = models.entry(&e.key).or_default();
                *sum = sum.plus(e.counts);
            }
            let models: Vec<Value> = models
                .into_iter()
                .map(|((provider, model), counts)| {
                    let mut out = Map::new();
                    if let Some(p) = provider {
                        out.insert("provider".into(), p.clone().into());
                    }
                    out.insert("model".into(), model.clone().into());
                    counts.put(&mut out);
                    Value::Object(out)
                })
                .collect();
            json!({"task": task, "models": models})
        })
        .collect();
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|((kind, name), g)| {
            let mut out = Map::new();
            out.insert("kind".into(), kind.into());
            out.insert("name".into(), name.into());
            out.insert("calls".into(), g.calls.into());
            g.counts.put(&mut out);
            Value::Object(out)
        })
        .collect();
    let mut usage = Map::new();
    usage.insert("tasks".into(), tasks.into());
    usage.insert("groups".into(), groups.into());
    if let Some(call) = latest {
        let mut out = Map::new();
        out.insert("task".into(), call.task.clone().into());
        out.insert("call".into(), call.call.clone().into());
        if let Some(p) = &call.entry.key.0 {
            out.insert("provider".into(), p.clone().into());
        }
        out.insert("model".into(), call.entry.key.1.clone().into());
        call.entry.counts.put(&mut out);
        if let Some(w) = call.context_window {
            out.insert("contextWindow".into(), w.into());
        }
        out.insert("by".into(), json!({"kind": call.by.0, "name": call.by.1}));
        usage.insert("latest".into(), Value::Object(out));
    }
    Some(Value::Object(usage))
}
