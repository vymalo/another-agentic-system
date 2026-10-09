//! Token usage in the projection (ADR 0056, `docs/api/agui.md` "Token usage").
//!
//! The fold of the `model_usage` and `model_usage_total` events that AG-UI's run accounting needs:
//! per A2A task, what is **known** of it (its latest totals, plus the calls reported after them; the
//! sum of its calls while it has none) and what an earlier run of it already **reported**; per run,
//! the tasks it touched. When a run closes, `RUN_FINISHED.usage` (or `RUN_ERROR.usage`) is, summed
//! per provider and model over the thread's agent's tasks the run touched, what is known less what
//! was reported: the task's latest totals for a run of its own, and only what a run added for a run
//! that resumes a task (AG-UI: "a run that resumes an interrupted one reports only the calls it made
//! itself"). An asked agent's task is said under its own subagent and is not in its asker's run.
//!
//! A function of the events alone, like the rest of the projector: a replay folds the same events
//! and closes its runs with the same usage.

use std::collections::{BTreeMap, BTreeSet};

use orch_agui_proto::TokenUsage;
use orch_core::{ModelTokens, ModelUsageData, ModelUsageTotalData, TokenCounts};

/// A provider and a model.
type Key = (Option<String>, String);

/// Counts that add and subtract: a part (reasoning, cached) is unknown until one entry says it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tally {
    input: u64,
    output: u64,
    total: u64,
    reasoning: Option<u64>,
    cached: Option<u64>,
    cache_write: Option<u64>,
}

fn add_part(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (Some(n), None) | (None, Some(n)) => Some(n),
        (None, None) => None,
    }
}

fn sub_part(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    a.map(|a| a.saturating_sub(b.unwrap_or(0)))
}

impl From<&TokenCounts> for Tally {
    fn from(c: &TokenCounts) -> Self {
        Tally {
            input: c.input_tokens,
            output: c.output_tokens,
            total: c.total_tokens,
            reasoning: c.reasoning_tokens,
            cached: c.cached_input_tokens,
            cache_write: c.cache_write_input_tokens,
        }
    }
}

impl Tally {
    fn plus(self, o: Tally) -> Tally {
        Tally {
            input: self.input.saturating_add(o.input),
            output: self.output.saturating_add(o.output),
            total: self.total.saturating_add(o.total),
            reasoning: add_part(self.reasoning, o.reasoning),
            cached: add_part(self.cached, o.cached),
            cache_write: add_part(self.cache_write, o.cache_write),
        }
    }

    /// What `self` has beyond `o`, never below zero.
    fn minus(self, o: Tally) -> Tally {
        Tally {
            input: self.input.saturating_sub(o.input),
            output: self.output.saturating_sub(o.output),
            total: self.total.saturating_sub(o.total),
            reasoning: sub_part(self.reasoning, o.reasoning),
            cached: sub_part(self.cached, o.cached),
            cache_write: sub_part(self.cache_write, o.cache_write),
        }
    }

    fn is_zero(&self) -> bool {
        self.input == 0
            && self.output == 0
            && self.total == 0
            && self.reasoning.unwrap_or(0) == 0
            && self.cached.unwrap_or(0) == 0
            && self.cache_write.unwrap_or(0) == 0
    }

    fn usage(self, (provider, model): Key) -> TokenUsage {
        TokenUsage {
            provider,
            model: Some(model),
            input_tokens: Some(self.input),
            output_tokens: Some(self.output),
            total_tokens: Some(self.total),
            reasoning_tokens: self.reasoning,
            cached_input_tokens: self.cached,
            cache_write_input_tokens: self.cache_write,
        }
    }
}

fn add_into(map: &mut BTreeMap<Key, Tally>, key: Key, tally: Tally) {
    let entry = map.entry(key).or_default();
    *entry = entry.plus(tally);
}

/// What the projection knows of one A2A task's usage.
#[derive(Debug, Clone, Default)]
struct TaskUsage {
    /// An asked agent's task: said under its ask, not in its asker's run.
    ask: bool,
    /// Its latest totals, once it said any.
    totals: Option<BTreeMap<Key, Tally>>,
    /// The calls reported since the latest totals (all of them while there are none).
    since: BTreeMap<Key, Tally>,
    /// What runs closed so far said of it.
    reported: BTreeMap<Key, Tally>,
}

impl TaskUsage {
    /// The latest totals plus the calls after them.
    fn known(&self) -> BTreeMap<Key, Tally> {
        let mut known = self.totals.clone().unwrap_or_default();
        for (key, tally) in &self.since {
            add_into(&mut known, key.clone(), *tally);
        }
        known
    }
}

/// Whether a step path is an asked agent's (`ask-<n>`, ADR 0026).
pub(crate) fn is_ask_path(path: &[String]) -> bool {
    path.iter().any(|id| ask_number(id).is_some())
}

/// The number of the ask whose step id is `id`.
pub(crate) fn ask_number(id: &str) -> Option<u32> {
    id.strip_prefix("ask-")?.parse().ok()
}

fn key_of(provider: Option<&String>, model: &str) -> Key {
    (provider.cloned(), model.to_owned())
}

/// The usage of the thread folded so far, and the tasks the open run touched.
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageFold {
    tasks: BTreeMap<String, TaskUsage>,
    /// The tasks the open run touched, and the providers and models of each it heard of.
    touched: BTreeMap<String, BTreeSet<Key>>,
}

impl UsageFold {
    /// A call; `in_run` when a run is open to count it in.
    pub(crate) fn call(&mut self, d: &ModelUsageData, in_run: bool) {
        let key = key_of(d.provider.as_ref(), &d.model);
        let task = self.tasks.entry(d.task.clone()).or_default();
        task.ask |= is_ask_path(&d.path);
        add_into(&mut task.since, key.clone(), Tally::from(&d.tokens));
        if in_run {
            self.touched.entry(d.task.clone()).or_default().insert(key);
        }
    }

    /// A task's totals; `in_run` when a run is open to count them in.
    pub(crate) fn total(&mut self, d: &ModelUsageTotalData, in_run: bool) {
        let task = self.tasks.entry(d.task.clone()).or_default();
        task.ask |= is_ask_path(&d.path);
        let totals: BTreeMap<Key, Tally> = d
            .totals
            .iter()
            .map(|m: &ModelTokens| {
                (
                    key_of(m.provider.as_ref(), &m.model),
                    Tally::from(&m.tokens),
                )
            })
            .collect();
        let keys: Vec<Key> = totals.keys().cloned().collect();
        task.totals = Some(totals);
        task.since.clear();
        if in_run {
            self.touched.entry(d.task.clone()).or_default().extend(keys);
        }
    }

    /// The open run closes: its usage (`None` when it touched none of the thread's agent's tasks),
    /// one entry per provider and model in their order, and what each task reported moves on.
    pub(crate) fn close_run(&mut self) -> Option<Vec<TokenUsage>> {
        let touched = std::mem::take(&mut self.touched);
        let mut sum: BTreeMap<Key, Tally> = BTreeMap::new();
        let mut said = false;
        for (id, heard) in touched {
            let Some(task) = self.tasks.get_mut(&id) else {
                continue;
            };
            if task.ask {
                continue;
            }
            let known = task.known();
            for (key, now) in &known {
                let delta = now.minus(task.reported.get(key).copied().unwrap_or_default());
                if !delta.is_zero() || heard.contains(key) {
                    add_into(&mut sum, key.clone(), delta);
                    said = true;
                }
            }
            task.reported = known;
        }
        said.then(|| sum.into_iter().map(|(key, t)| t.usage(key)).collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn counts(input: u64, output: u64) -> TokenCounts {
        TokenCounts {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
            ..TokenCounts::default()
        }
    }

    fn call(task: &str, model: &str, input: u64, output: u64, path: &[&str]) -> ModelUsageData {
        ModelUsageData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            call: format!("{task}-{input}"),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
            provider: Some("openai".into()),
            model: model.into(),
            tokens: counts(input, output),
            context_window: None,
        }
    }

    fn total(task: &str, entries: &[(&str, u64, u64)], path: &[&str]) -> ModelUsageTotalData {
        ModelUsageTotalData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
            totals: entries
                .iter()
                .map(|(model, input, output)| ModelTokens {
                    provider: Some("openai".into()),
                    model: (*model).into(),
                    tokens: counts(*input, *output),
                })
                .collect(),
        }
    }

    fn said(usage: Option<Vec<TokenUsage>>) -> Vec<(String, u64, u64)> {
        usage
            .unwrap_or_default()
            .into_iter()
            .map(|u| {
                (
                    u.model.unwrap(),
                    u.input_tokens.unwrap(),
                    u.output_tokens.unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn a_run_says_its_tasks_totals_and_the_sum_of_its_calls_without_them() {
        let mut fold = UsageFold::default();
        fold.call(&call("t1", "m", 10, 1, &[]), true);
        fold.call(&call("t1", "m", 20, 2, &["t1/tool:c2"]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 30, 3)]);
        // a run that touched nothing says nothing
        assert_eq!(fold.close_run(), None);
        let mut fold = UsageFold::default();
        fold.call(&call("t1", "m", 10, 1, &[]), true);
        // lost calls: the totals are the record
        fold.total(&total("t1", &[("m", 50, 5), ("small", 4, 1)], &[]), true);
        assert_eq!(
            said(fold.close_run()),
            [("m".to_owned(), 50, 5), ("small".to_owned(), 4, 1)]
        );
    }

    #[test]
    fn a_run_that_resumes_a_task_says_only_what_it_added() {
        let mut fold = UsageFold::default();
        fold.call(&call("t1", "m", 10, 1, &[]), true);
        fold.total(&total("t1", &[("m", 10, 1)], &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 10, 1)]);
        // the person answers: the task goes on, and its totals are all of its calls so far
        fold.call(&call("t1", "m", 30, 3, &[]), true);
        fold.total(&total("t1", &[("m", 40, 4)], &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 30, 3)]);
    }

    #[test]
    fn a_run_that_a_message_ended_says_its_calls_and_the_next_the_rest_of_the_totals() {
        let mut fold = UsageFold::default();
        fold.call(&call("t1", "m", 10, 1, &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 10, 1)]);
        fold.call(&call("t1", "m", 20, 2, &[]), true);
        fold.total(&total("t1", &[("m", 30, 3)], &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 20, 2)]);
    }

    #[test]
    fn an_asked_agents_task_is_not_in_its_askers_run() {
        let mut fold = UsageFold::default();
        fold.call(&call("ask-task", "m", 10, 1, &["ask-1"]), true);
        fold.total(&total("ask-task", &[("m", 10, 1)], &["ask-1"]), true);
        assert_eq!(fold.close_run(), None);
        fold.call(&call("t1", "m", 5, 1, &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 5, 1)]);
    }

    #[test]
    fn what_came_while_no_run_was_open_is_said_by_the_next_run_of_the_task() {
        let mut fold = UsageFold::default();
        fold.call(&call("t1", "m", 10, 1, &[]), false);
        assert_eq!(fold.close_run(), None);
        fold.call(&call("t1", "m", 5, 1, &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 15, 2)]);
    }

    #[test]
    fn a_call_that_cost_nothing_is_still_said() {
        let mut fold = UsageFold::default();
        fold.call(&call("t1", "m", 0, 0, &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 0, 0)]);
    }

    #[test]
    fn totals_that_fall_below_what_was_said_never_go_below_zero() {
        let mut fold = UsageFold::default();
        fold.call(&call("t1", "m", 10, 1, &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 10, 1)]);
        fold.total(&total("t1", &[("m", 4, 0)], &[]), true);
        assert_eq!(said(fold.close_run()), [("m".to_owned(), 0, 0)]);
    }
}
