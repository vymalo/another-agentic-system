//! Token usage of model calls (ADR 0056, `docs/api/usage-v1.md`).
//!
//! An agent that lists `usage/v1` reports the tokens of each model call it made (a **call
//! report**, [`UsageCall`]) and, when its task ends or pauses, the task's **totals**
//! ([`UsageTotals`]). The counts follow AG-UI 1.0's `TokenUsage` accounting ([`TokenCounts`]):
//! `inputTokens` and `outputTokens` are totals, `totalTokens` their sum, and the optional counts
//! are parts of them.
//!
//! A report is **data from an agent**: [`UsageCall::parse`] and [`UsageTotals::parse`] are the
//! door an adapter reads the extension's JSON through, and [`UsageCall::check`] /
//! [`UsageTotals::check`] the same rules again where the core records it, whatever adapter built
//! it. What does not pass is an [`UsageInvalid`]: dropped and counted, never logged and never a
//! task failure.
//!
//! The core logs a valid call report as a `model_usage` event ([`ModelUsageData`]) with the
//! **path** of the step the call ran under (the rule of `agent_step`: the step's own path and the
//! step, when the job's step ledger holds it open; empty, the agent's own call, when it does not),
//! and a valid total as a `model_usage_total` ([`ModelUsageTotalData`]). A job logs at most
//! [`MAX_USAGE_CALLS_PER_JOB`] call reports ([`UsageLedger`], `Job.usage`); totals come at most once
//! per turn of a task and are not capped. A report about an asked agent's task
//! ([`Input::AskUsage`](crate::Input::AskUsage)) is attributed to the asked agent, under the path
//! `["ask-<n>"]`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ask::ask_step_id;
use crate::event::{Actor, EventBody};
use crate::gate::Job;
use crate::step::MAX_STEP_ID_BYTES;
use crate::thread::ThreadState;
use crate::transition::{Command, append};

/// Most call reports one job logs; past it a report is dropped and counted (`Job.usage`).
pub const MAX_USAGE_CALLS_PER_JOB: u32 = 10_000;
/// Longest call id, provider, model and step id an agent may send, in bytes.
pub const MAX_USAGE_LABEL_BYTES: usize = 128;
/// Longest A2A task id a report may name, in bytes.
pub const MAX_USAGE_TASK_BYTES: usize = 256;
/// Most entries of a task's totals: one per provider and model.
pub const MAX_USAGE_TOTALS: usize = 32;
/// The largest count: 2^53 - 1, the range a JSON number survives a round trip in (AG-UI's bound).
pub const MAX_TOKEN_COUNT: u64 = 9_007_199_254_740_991;

/// Why a usage report was not taken. Closed (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum UsageInvalid {
    /// The entry under the extension's URI is not a JSON object.
    #[error("the report is not an object")]
    NotAnObject,
    /// A required member is missing (`call`, `model`, a count, `totals`).
    #[error("a required member is missing")]
    Missing,
    /// A member has the wrong type (a label that is not a string, `totals` that is not an array).
    #[error("a member has the wrong type")]
    WrongType,
    /// A label, the call id, the step id or the task is empty, too long or has a control character.
    #[error("a label or an id is empty, too long or has a control character")]
    BadText,
    /// A count is not a whole number from 0 to 2^53 - 1.
    #[error("a count is not a whole number from 0 to 2^53 - 1")]
    BadCount,
    /// `totalTokens` is not `inputTokens` plus `outputTokens`.
    #[error("totalTokens is not inputTokens plus outputTokens")]
    TotalNotSum,
    /// More than [`MAX_USAGE_TOTALS`] totals.
    #[error("more than 32 totals")]
    TooManyTotals,
    /// Two totals name the same provider and model.
    #[error("two totals name the same provider and model")]
    RepeatedModel,
}

impl UsageInvalid {
    /// The wire spelling, also a log field.
    pub const fn as_str(self) -> &'static str {
        match self {
            UsageInvalid::NotAnObject => "not_an_object",
            UsageInvalid::Missing => "missing",
            UsageInvalid::WrongType => "wrong_type",
            UsageInvalid::BadText => "bad_text",
            UsageInvalid::BadCount => "bad_count",
            UsageInvalid::TotalNotSum => "total_not_sum",
            UsageInvalid::TooManyTotals => "too_many_totals",
            UsageInvalid::RepeatedModel => "repeated_model",
        }
    }
}

/// The counts of one call or one provider and model, in AG-UI 1.0's `TokenUsage` accounting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    /// Every prompt token (a total).
    pub input_tokens: u64,
    /// Every completion token, reasoning included (a total).
    pub output_tokens: u64,
    /// `input_tokens` plus `output_tokens`.
    pub total_tokens: u64,
    /// Output tokens spent on reasoning: part of `output_tokens`. Absent when the provider does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Input tokens read from a provider cache: part of `input_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    /// Input tokens written to a provider cache: part of `input_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
}

impl TokenCounts {
    /// Whether every count is in range and `total_tokens` is the sum.
    pub fn check(&self) -> Result<(), UsageInvalid> {
        let counts = [
            Some(self.input_tokens),
            Some(self.output_tokens),
            Some(self.total_tokens),
            self.reasoning_tokens,
            self.cached_input_tokens,
            self.cache_write_input_tokens,
        ];
        if counts.into_iter().flatten().any(|n| n > MAX_TOKEN_COUNT) {
            return Err(UsageInvalid::BadCount);
        }
        if self.input_tokens.checked_add(self.output_tokens) != Some(self.total_tokens) {
            return Err(UsageInvalid::TotalNotSum);
        }
        Ok(())
    }

    /// The counts of an entry (`inputTokens`, `outputTokens`, `totalTokens` required, the parts
    /// optional), checked.
    fn parse(entry: &Map<String, Value>) -> Result<TokenCounts, UsageInvalid> {
        let counts = TokenCounts {
            input_tokens: required_count(entry, "inputTokens")?,
            output_tokens: required_count(entry, "outputTokens")?,
            total_tokens: required_count(entry, "totalTokens")?,
            reasoning_tokens: optional_count(entry, "reasoningTokens")?,
            cached_input_tokens: optional_count(entry, "cachedInputTokens")?,
            cache_write_input_tokens: optional_count(entry, "cacheWriteInputTokens")?,
        };
        counts.check()?;
        Ok(counts)
    }
}

/// One provider and model of a task's totals: the AG-UI `TokenUsage` shape, with a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokens {
    /// The provider as the agent knows it (lower case, e.g. `openai`), when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The model as configured.
    pub model: String,
    /// Its counts.
    #[serde(flatten)]
    pub tokens: TokenCounts,
}

/// A call report as an adapter read it (`usage-v1.md` section 3): not yet trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCall {
    /// The A2A task the call belongs to.
    pub task: String,
    /// The agent's id for the call, unique within the task.
    pub call: String,
    /// The step the call ran under, made unique within the thread like a step's id
    /// (`<task>/<stepId>`); `None` for the agent's own call.
    pub step: Option<String>,
    /// The provider, when the agent says.
    pub provider: Option<String>,
    /// The model.
    pub model: String,
    /// The counts.
    pub tokens: TokenCounts,
    /// The model's context window in tokens, as the deployment configured it.
    pub context_window: Option<u64>,
}

impl UsageCall {
    /// The call report `entry` (the value under the extension's URI in a status update's
    /// metadata) of the task `task`. Unknown members are ignored; a number written as a double
    /// (`10.0`, which an A2A server may hand back for `10`) counts when it is whole.
    pub fn parse(task: &str, entry: &Value) -> Result<UsageCall, UsageInvalid> {
        let entry = entry.as_object().ok_or(UsageInvalid::NotAnObject)?;
        let call = label(entry, "call")?.ok_or(UsageInvalid::Missing)?;
        let model = label(entry, "model")?.ok_or(UsageInvalid::Missing)?;
        let step = optional_label(entry, "stepId")?.map(|id| format!("{task}/{id}"));
        let report = UsageCall {
            task: task.to_owned(),
            call,
            step,
            provider: optional_label(entry, "provider")?,
            model,
            tokens: TokenCounts::parse(entry)?,
            context_window: optional_count(entry, "contextWindow")?,
        };
        report.check()?;
        Ok(report)
    }

    /// The rules of [`UsageCall::parse`] on a report already built: the core's door.
    pub fn check(&self) -> Result<(), UsageInvalid> {
        check_task(&self.task)?;
        if !is_label(&self.call, MAX_USAGE_LABEL_BYTES)
            || !is_label(&self.model, MAX_USAGE_LABEL_BYTES)
        {
            return Err(UsageInvalid::BadText);
        }
        if let Some(provider) = &self.provider
            && !is_label(provider, MAX_USAGE_LABEL_BYTES)
        {
            return Err(UsageInvalid::BadText);
        }
        if let Some(step) = &self.step
            && !is_label(step, MAX_STEP_ID_BYTES)
        {
            return Err(UsageInvalid::BadText);
        }
        if self.context_window.is_some_and(|n| n > MAX_TOKEN_COUNT) {
            return Err(UsageInvalid::BadCount);
        }
        self.tokens.check()
    }
}

/// A task's totals as an adapter read them (`usage-v1.md` section 4): not yet trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    /// The A2A task.
    pub task: String,
    /// One entry per provider and model, covering every model call the task made.
    pub totals: Vec<ModelTokens>,
}

impl UsageTotals {
    /// The totals `entry` (the value under the extension's URI in a task's metadata) of the task
    /// `task`: `{totals: [...]}`, at most [`MAX_USAGE_TOTALS`] entries, each a provider and model
    /// once.
    pub fn parse(task: &str, entry: &Value) -> Result<UsageTotals, UsageInvalid> {
        let entry = entry.as_object().ok_or(UsageInvalid::NotAnObject)?;
        let items = match entry.get("totals") {
            None | Some(Value::Null) => return Err(UsageInvalid::Missing),
            Some(Value::Array(items)) => items,
            Some(_) => return Err(UsageInvalid::WrongType),
        };
        if items.len() > MAX_USAGE_TOTALS {
            return Err(UsageInvalid::TooManyTotals);
        }
        let totals = items
            .iter()
            .map(|item| {
                let item = item.as_object().ok_or(UsageInvalid::WrongType)?;
                Ok(ModelTokens {
                    provider: optional_label(item, "provider")?,
                    model: label(item, "model")?.ok_or(UsageInvalid::Missing)?,
                    tokens: TokenCounts::parse(item)?,
                })
            })
            .collect::<Result<Vec<_>, UsageInvalid>>()?;
        let report = UsageTotals {
            task: task.to_owned(),
            totals,
        };
        report.check()?;
        Ok(report)
    }

    /// The rules of [`UsageTotals::parse`] on totals already built: the core's door.
    pub fn check(&self) -> Result<(), UsageInvalid> {
        check_task(&self.task)?;
        if self.totals.len() > MAX_USAGE_TOTALS {
            return Err(UsageInvalid::TooManyTotals);
        }
        let mut seen = BTreeSet::new();
        for entry in &self.totals {
            if !is_label(&entry.model, MAX_USAGE_LABEL_BYTES)
                || entry
                    .provider
                    .as_deref()
                    .is_some_and(|p| !is_label(p, MAX_USAGE_LABEL_BYTES))
            {
                return Err(UsageInvalid::BadText);
            }
            entry.tokens.check()?;
            if !seen.insert((entry.provider.as_deref(), entry.model.as_str())) {
                return Err(UsageInvalid::RepeatedModel);
            }
        }
        Ok(())
    }
}

/// What an adapter read of `usage/v1`, already through the door.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageUpdate {
    /// One model call's tokens.
    Call(UsageCall),
    /// A task's totals, as it ended or paused.
    Total(UsageTotals),
}

/// `data` of a `model_usage`: one model call, labels and numbers only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageData {
    /// The job of the thread the call was made in.
    pub job: u32,
    /// The agent that made it: the thread's agent, or an asked agent.
    pub agent: String,
    /// The A2A task it belongs to.
    pub task: String,
    /// The agent's id for the call, unique within the task.
    pub call: String,
    /// The step path it ran under: empty for the agent's own call; the sub-agent step's path and
    /// the step for a call under one; `["ask-<n>"]` for an asked agent's.
    #[serde(default)]
    pub path: Vec<String>,
    /// The provider, when the agent said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The model.
    pub model: String,
    /// The counts.
    #[serde(flatten)]
    pub tokens: TokenCounts,
    /// The model's context window in tokens, when the deployment configured one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// `data` of a `model_usage_total`: a task's totals when it ended or paused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageTotalData {
    /// The job of the thread the task belongs to.
    pub job: u32,
    /// The agent whose task it is.
    pub agent: String,
    /// The A2A task.
    pub task: String,
    /// `["ask-<n>"]` for an asked agent's task; absent for the thread's agent's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    /// One entry per provider and model, every call of the task included.
    pub totals: Vec<ModelTokens>,
}

/// The job's count of its usage (`Job.usage`): how many call reports it logged and how many it
/// dropped past [`MAX_USAGE_CALLS_PER_JOB`]. Belongs to the job: [`Job::next`] forgets it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageLedger {
    #[serde(skip_serializing_if = "is_zero")]
    calls: u32,
    #[serde(skip_serializing_if = "is_zero")]
    dropped: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl UsageLedger {
    /// A ledger that logged `calls` call reports and dropped `dropped` (what a store reads back).
    pub const fn of(calls: u32, dropped: u32) -> Self {
        UsageLedger { calls, dropped }
    }

    /// A ledger that has seen no report (what the log leaves out).
    pub fn is_empty(&self) -> bool {
        self.calls == 0 && self.dropped == 0
    }

    /// How many call reports the job logged.
    pub fn calls(&self) -> u32 {
        self.calls
    }

    /// How many call reports the job dropped because it had logged as many as it may.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }
}

/// Records one usage report: the event it gives, none when it is dropped.
///
/// Taken while the job is not finished (`queued`, `working`, `blocked`, `verifying`): tokens spent
/// are spent, and a report moves no state. A finished thread takes none: callers refuse those first
/// (`TransitionError::InvalidInState`), and here they change nothing. A report that fails the door
/// again is dropped; a call report past [`MAX_USAGE_CALLS_PER_JOB`] is dropped and counted.
///
/// `ask`: the ask whose task the report is about (`Input::AskUsage`); its path is `["ask-<n>"]`.
pub(crate) fn record_usage(
    state: ThreadState,
    job: &mut Job,
    actor: Actor,
    update: &UsageUpdate,
    ask: Option<u32>,
) -> (ThreadState, Vec<Command>) {
    match state {
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Blocked
        | ThreadState::Verifying => {}
        ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
            return (state, Vec::new());
        }
    }
    let ask_path = || ask.map(|n| vec![ask_step_id(n)]);
    let body = match update {
        UsageUpdate::Call(call) => {
            if call.check().is_err() {
                return (state, Vec::new());
            }
            if job.usage.calls >= MAX_USAGE_CALLS_PER_JOB {
                job.usage.dropped = job.usage.dropped.saturating_add(1);
                return (state, Vec::new());
            }
            job.usage.calls += 1;
            let path = ask_path().unwrap_or_else(|| {
                call.step
                    .as_deref()
                    .map(|step| job.steps.path_into(step))
                    .unwrap_or_default()
            });
            EventBody::ModelUsage(Box::new(ModelUsageData {
                job: job.number,
                agent: actor.name.clone(),
                task: call.task.clone(),
                call: call.call.clone(),
                path,
                provider: call.provider.clone(),
                model: call.model.clone(),
                tokens: call.tokens,
                context_window: call.context_window,
            }))
        }
        UsageUpdate::Total(totals) => {
            if totals.check().is_err() {
                return (state, Vec::new());
            }
            EventBody::ModelUsageTotal(ModelUsageTotalData {
                job: job.number,
                agent: actor.name.clone(),
                task: totals.task.clone(),
                path: ask_path().unwrap_or_default(),
                totals: totals.totals.clone(),
            })
        }
    };
    (state, vec![append(actor, body)])
}

/// A label or an id: 1 to `max` bytes, not blank, with no control character.
fn is_label(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

fn check_task(task: &str) -> Result<(), UsageInvalid> {
    if is_label(task, MAX_USAGE_TASK_BYTES) {
        Ok(())
    } else {
        Err(UsageInvalid::BadText)
    }
}

/// A label member: `None` when absent or `null`, the text when it is a usable label.
fn label(entry: &Map<String, Value>, key: &str) -> Result<Option<String>, UsageInvalid> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if is_label(s, MAX_USAGE_LABEL_BYTES) => Ok(Some(s.clone())),
        Some(Value::String(_)) => Err(UsageInvalid::BadText),
        Some(_) => Err(UsageInvalid::WrongType),
    }
}

/// An optional label: an empty string is no label, as an absent one.
fn optional_label(entry: &Map<String, Value>, key: &str) -> Result<Option<String>, UsageInvalid> {
    match entry.get(key) {
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        _ => label(entry, key),
    }
}

/// A count: a whole number from 0 to [`MAX_TOKEN_COUNT`], written as an integer or as a double
/// with no fraction.
fn count_of(value: &Value) -> Result<u64, UsageInvalid> {
    if let Some(n) = value.as_u64() {
        return (n <= MAX_TOKEN_COUNT)
            .then_some(n)
            .ok_or(UsageInvalid::BadCount);
    }
    let Some(f) = value.as_f64() else {
        return Err(match value {
            Value::Number(_) => UsageInvalid::BadCount,
            _ => UsageInvalid::WrongType,
        });
    };
    // 2^53 - 1 is exact as a double, so the comparison is too
    if f.is_finite() && f >= 0.0 && f.fract() == 0.0 && f <= MAX_TOKEN_COUNT as f64 {
        Ok(f as u64)
    } else {
        Err(UsageInvalid::BadCount)
    }
}

fn required_count(entry: &Map<String, Value>, key: &str) -> Result<u64, UsageInvalid> {
    match entry.get(key) {
        None | Some(Value::Null) => Err(UsageInvalid::Missing),
        Some(value) => count_of(value),
    }
}

fn optional_count(entry: &Map<String, Value>, key: &str) -> Result<Option<u64>, UsageInvalid> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => count_of(value).map(Some),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn report() -> Value {
        json!({"call": "c7", "stepId": "tool:call_2", "provider": "openai", "model": "glm-5.3",
               "inputTokens": 41250, "outputTokens": 812, "totalTokens": 42062,
               "reasoningTokens": 300, "cachedInputTokens": 38000, "contextWindow": 131072})
    }

    #[test]
    fn the_contracts_example_is_a_call_under_a_step_made_unique_by_the_task() {
        let call = UsageCall::parse("t1", &report()).unwrap();
        assert_eq!(call.task, "t1");
        assert_eq!(call.call, "c7");
        assert_eq!(call.step.as_deref(), Some("t1/tool:call_2"));
        assert_eq!(call.provider.as_deref(), Some("openai"));
        assert_eq!(call.model, "glm-5.3");
        assert_eq!(call.tokens.input_tokens, 41250);
        assert_eq!(call.tokens.output_tokens, 812);
        assert_eq!(call.tokens.total_tokens, 42062);
        assert_eq!(call.tokens.reasoning_tokens, Some(300));
        assert_eq!(call.tokens.cached_input_tokens, Some(38000));
        assert_eq!(call.tokens.cache_write_input_tokens, None);
        assert_eq!(call.context_window, Some(131_072));
    }

    #[test]
    fn only_the_required_members_are_required_and_unknown_ones_are_ignored() {
        let call = UsageCall::parse(
            "t1",
            &json!({"call": "c1", "model": "m", "inputTokens": 0, "outputTokens": 0,
                    "totalTokens": 0, "somethingNew": [1, 2]}),
        )
        .unwrap();
        assert_eq!(call.step, None);
        assert_eq!(call.provider, None);
        assert_eq!(call.context_window, None);
        assert_eq!(call.tokens, TokenCounts::default());
    }

    #[test]
    fn a_whole_number_written_as_a_double_counts_and_a_fraction_does_not() {
        let mut entry = report();
        entry["inputTokens"] = json!(41250.0);
        entry["contextWindow"] = json!(131072.0);
        let call = UsageCall::parse("t1", &entry).unwrap();
        assert_eq!(call.tokens.input_tokens, 41250);
        assert_eq!(call.context_window, Some(131_072));
        entry["inputTokens"] = json!(41250.5);
        assert_eq!(UsageCall::parse("t1", &entry), Err(UsageInvalid::BadCount));
    }

    #[test]
    fn every_broken_report_is_invalid() {
        let cases: Vec<(Value, UsageInvalid)> = vec![
            (json!("c1"), UsageInvalid::NotAnObject),
            (
                json!({"model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::Missing,
            ),
            (
                json!({"call": "c", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::Missing,
            ),
            (
                json!({"call": "c", "model": "m", "outputTokens": 1, "totalTokens": 1}),
                UsageInvalid::Missing,
            ),
            (
                json!({"call": "c", "model": "m", "inputTokens": 1, "outputTokens": 1}),
                UsageInvalid::Missing,
            ),
            (
                json!({"call": 7, "model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::WrongType,
            ),
            (
                json!({"call": "c", "model": "", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::BadText,
            ),
            (
                json!({"call": "c\u{0}", "model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::BadText,
            ),
            (
                json!({"call": "x".repeat(129), "model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::BadText,
            ),
            (
                json!({"call": "c", "model": "m".repeat(129), "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::BadText,
            ),
            (
                json!({"call": "c", "model": "m", "provider": 3, "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::WrongType,
            ),
            (
                json!({"call": "c", "model": "m", "stepId": {}, "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::WrongType,
            ),
            (
                json!({"call": "c", "model": "m", "inputTokens": -1, "outputTokens": 1, "totalTokens": 0}),
                UsageInvalid::BadCount,
            ),
            (
                json!({"call": "c", "model": "m", "inputTokens": "1", "outputTokens": 1, "totalTokens": 2}),
                UsageInvalid::WrongType,
            ),
            (
                json!({"call": "c", "model": "m", "inputTokens": 9_007_199_254_740_992_u64, "outputTokens": 0, "totalTokens": 9_007_199_254_740_992_u64}),
                UsageInvalid::BadCount,
            ),
            (
                json!({"call": "c", "model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 3}),
                UsageInvalid::TotalNotSum,
            ),
            (
                json!({"call": "c", "model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2, "reasoningTokens": 1.5}),
                UsageInvalid::BadCount,
            ),
            (
                json!({"call": "c", "model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2, "contextWindow": -4}),
                UsageInvalid::BadCount,
            ),
        ];
        for (entry, why) in cases {
            assert_eq!(UsageCall::parse("t1", &entry), Err(why), "{entry}");
        }
    }

    #[test]
    fn the_largest_count_is_two_to_the_fifty_three_minus_one() {
        let entry = json!({"call": "c", "model": "m", "inputTokens": MAX_TOKEN_COUNT,
                           "outputTokens": 0, "totalTokens": MAX_TOKEN_COUNT});
        assert!(UsageCall::parse("t1", &entry).is_ok());
    }

    #[test]
    fn totals_are_one_entry_per_provider_and_model() {
        let totals = UsageTotals::parse(
            "t1",
            &json!({"totals": [
                {"provider": "openai", "model": "glm-5.3", "inputTokens": 512000, "outputTokens": 9100,
                 "totalTokens": 521100, "reasoningTokens": 2400, "cachedInputTokens": 470000},
                {"model": "small", "inputTokens": 10, "outputTokens": 2, "totalTokens": 12}
            ]}),
        )
        .unwrap();
        assert_eq!(totals.totals.len(), 2);
        assert_eq!(totals.totals[0].tokens.total_tokens, 521_100);
        assert_eq!(totals.totals[1].provider, None);
        let repeated = json!({"totals": [
            {"model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2},
            {"model": "m", "inputTokens": 3, "outputTokens": 1, "totalTokens": 4}
        ]});
        assert_eq!(
            UsageTotals::parse("t1", &repeated),
            Err(UsageInvalid::RepeatedModel)
        );
        let many: Vec<Value> = (0..33)
            .map(|i| json!({"model": format!("m{i}"), "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}))
            .collect();
        assert_eq!(
            UsageTotals::parse("t1", &json!({"totals": many})),
            Err(UsageInvalid::TooManyTotals)
        );
        assert_eq!(
            UsageTotals::parse("t1", &json!({})),
            Err(UsageInvalid::Missing)
        );
        assert_eq!(
            UsageTotals::parse("t1", &json!({"totals": {}})),
            Err(UsageInvalid::WrongType)
        );
        assert_eq!(
            UsageTotals::parse(
                "t1",
                &json!({"totals": [{"model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 1}]})
            ),
            Err(UsageInvalid::TotalNotSum)
        );
        assert!(
            UsageTotals::parse("t1", &json!({"totals": []}))
                .unwrap()
                .totals
                .is_empty()
        );
    }

    #[test]
    fn an_event_reads_back_the_shape_it_was_written_in() {
        let data = ModelUsageData {
            job: 1,
            agent: "coder".into(),
            task: "t1".into(),
            call: "c1".into(),
            path: vec![],
            provider: None,
            model: "m".into(),
            tokens: TokenCounts {
                input_tokens: 5,
                output_tokens: 2,
                total_tokens: 7,
                ..TokenCounts::default()
            },
            context_window: Some(100),
        };
        let wire = serde_json::to_value(&data).unwrap();
        assert_eq!(
            wire,
            json!({"job": 1, "agent": "coder", "task": "t1", "call": "c1", "path": [], "model": "m",
                   "inputTokens": 5, "outputTokens": 2, "totalTokens": 7, "contextWindow": 100})
        );
        assert_eq!(
            serde_json::from_value::<ModelUsageData>(wire).unwrap(),
            data
        );
    }
}
