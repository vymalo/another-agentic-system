//! The web's usage fold (`web/src/features/chat/lib/usage.ts`: `foldUsage` and `summarize`) as the
//! carry's reference: the same rules, written again in plain Rust over the frames' JSON values, with
//! the one addition the carry needs, a state that starts from the carry of a page. If the carry and
//! this agree on every golden and every generated log, a page and its carry give the web the state a
//! replay of the whole thread would.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};

type Counts = [Option<u64>; 6];

fn label(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn count(v: Option<&Value>) -> Option<u64> {
    v.and_then(Value::as_u64).filter(|n| *n < (1 << 53))
}

fn counts(v: &Map<String, Value>) -> Option<Counts> {
    Some([
        Some(count(v.get("inputTokens"))?),
        Some(count(v.get("outputTokens"))?),
        Some(count(v.get("totalTokens"))?),
        count(v.get("reasoningTokens")),
        count(v.get("cachedInputTokens")),
        count(v.get("cacheWriteInputTokens")),
    ])
}

fn plus(a: Counts, b: Counts) -> Counts {
    let mut out = [None; 6];
    for i in 0..6 {
        out[i] = match (a[i], b[i]) {
            (None, None) => None,
            (x, y) => Some(x.unwrap_or(0) + y.unwrap_or(0)),
        };
    }
    out
}

const ZERO: Counts = [Some(0), Some(0), Some(0), None, None, None];

type Model = (Option<String>, String);

#[derive(Clone, Debug)]
struct Entry {
    model: Model,
    counts: Counts,
}

fn entry(v: &Map<String, Value>) -> Option<Entry> {
    Some(Entry {
        model: (label(v.get("provider")), label(v.get("model"))?),
        counts: counts(v)?,
    })
}

#[derive(Clone, Debug)]
struct Call {
    task: String,
    call: String,
    entry: Entry,
    window: Option<u64>,
    by: (String, String),
}

fn call(value: &Value) -> Option<Call> {
    let v = value.as_object()?;
    let agent = label(v.get("agent")).unwrap_or_default();
    let by = match v.get("by").and_then(Value::as_object) {
        Some(b) => match label(b.get("kind")).as_deref() {
            Some(k @ ("agent" | "subagent" | "ask")) => {
                (k.to_owned(), label(b.get("name")).unwrap_or(agent))
            }
            _ => ("agent".to_owned(), agent),
        },
        None => ("agent".to_owned(), agent),
    };
    Some(Call {
        task: label(v.get("task"))?,
        call: label(v.get("call"))?,
        entry: entry(v)?,
        window: count(v.get("contextWindow")).filter(|n| *n > 0),
        by,
    })
}

/// The state of `usage.ts` (`ThreadUsage`), plus what a carry seeds it with.
#[derive(Default, Clone)]
pub struct Usage {
    log: Vec<Call>,
    seen: HashSet<(String, String)>,
    /// A task's latest totals and how many calls the state had when they came.
    totals: BTreeMap<String, (Vec<Entry>, usize)>,
    base_groups: Vec<((String, String), u64, Counts)>,
    base_latest: Option<Call>,
}

/// What the screen shows of a state.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub latest: Option<(String, String)>,
    pub fill: Option<(u64, u64)>,
    pub models: Vec<(Model, Counts)>,
    pub groups: Vec<((String, String), u64, Counts)>,
}

impl Usage {
    /// The state the web starts from when it holds a page and the carry that came with it.
    pub fn from_carry(carry: Option<&Value>) -> Usage {
        let mut state = Usage::default();
        let Some(carry) = carry.and_then(Value::as_object) else {
            return state;
        };
        for task in carry
            .get("tasks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = label(task.get("task")) else {
                continue;
            };
            let models = task
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|m| m.as_object().and_then(entry))
                .collect();
            // a synthetic total that covers no call of the state, and that a real one replaces
            state.totals.insert(id, (models, 0));
        }
        for g in carry
            .get("groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(g) = g.as_object() else { continue };
            let (Some(kind), Some(name), Some(calls), Some(c)) = (
                label(g.get("kind")),
                label(g.get("name")),
                count(g.get("calls")),
                counts(g),
            ) else {
                continue;
            };
            state.base_groups.push(((kind, name), calls, c));
        }
        state.base_latest = carry.get("latest").and_then(call);
        state
    }

    /// Folds one `CUSTOM` event.
    pub fn fold(&mut self, name: &str, value: &Value) {
        match name {
            "vymalo.usage" => {
                let Some(c) = call(value) else { return };
                if !self.seen.insert((c.task.clone(), c.call.clone())) {
                    return;
                }
                self.log.push(c);
            }
            "vymalo.usage_total" => {
                let Some(v) = value.as_object() else { return };
                let (Some(task), Some(totals)) = (
                    label(v.get("task")),
                    v.get("totals").and_then(Value::as_array),
                ) else {
                    return;
                };
                let entries: Vec<Entry> = totals
                    .iter()
                    .filter_map(|t| t.as_object().and_then(entry))
                    .collect();
                if let Some((known, _)) = self.totals.get(&task)
                    && same(known, &entries)
                {
                    return;
                }
                self.totals.insert(task, (entries, self.log.len()));
            }
            _ => {}
        }
    }

    pub fn summarize(&self) -> Summary {
        let latest = self
            .log
            .iter()
            .rev()
            .find(|c| c.by.0 == "agent")
            .or(self.base_latest.as_ref());
        let fill = latest.and_then(|c| {
            c.window
                .filter(|w| *w > 0)
                .map(|w| (c.entry.counts[0].unwrap().min(w), w))
        });
        let mut models: BTreeMap<Model, Counts> = BTreeMap::new();
        let mut add = |m: &Model, c: Counts| {
            let had = models.get(m).copied().unwrap_or(ZERO);
            models.insert(m.clone(), plus(had, c));
        };
        for (entries, _) in self.totals.values() {
            for e in entries {
                add(&e.model, e.counts);
            }
        }
        for (i, c) in self.log.iter().enumerate() {
            if let Some((_, after)) = self.totals.get(&c.task)
                && i < *after
            {
                continue;
            }
            add(&c.entry.model, c.entry.counts);
        }
        let mut groups: Vec<((String, String), u64, Counts)> = self.base_groups.clone();
        let mut place: HashMap<(String, String), usize> = groups
            .iter()
            .enumerate()
            .map(|(i, (k, _, _))| (k.clone(), i))
            .collect();
        for c in &self.log {
            let at = *place.entry(c.by.clone()).or_insert_with(|| {
                groups.push((c.by.clone(), 0, ZERO));
                groups.len() - 1
            });
            groups[at].1 += 1;
            groups[at].2 = plus(groups[at].2, c.entry.counts);
        }
        let rank = |kind: &str| match kind {
            "agent" => 0,
            "subagent" => 1,
            _ => 2,
        };
        groups.sort_by_key(|((kind, _), _, _)| rank(kind));
        let mut models: Vec<(Model, Counts)> = models.into_iter().collect();
        models.sort_by(|a, b| {
            b.1[2]
                .unwrap_or(0)
                .cmp(&a.1[2].unwrap_or(0))
                .then(a.0.1.cmp(&b.0.1))
        });
        Summary {
            latest: latest.map(|c| (c.task.clone(), c.call.clone())),
            fill,
            models,
            groups,
        }
    }
}

fn same(a: &[Entry], b: &[Entry]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.model == y.model && x.counts == y.counts)
}
