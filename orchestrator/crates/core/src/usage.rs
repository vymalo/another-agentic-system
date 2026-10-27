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
//!
//! The totals object may also carry the time a run's commands used in a **sandbox**
//! ([`SandboxUsage`], contract section 5). It is read apart from the token totals: a `sandbox`
//! that breaks the contract is left out and says why ([`UsageTotals::sandbox_rejected`], which the
//! application counts), and the token totals beside it are still taken. The members of the totals
//! object and of its entries that this module does not know are ignored.

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
/// Most entries of a task's sandbox usage: one per shape of the sandbox (`usage-v1.md` section 5).
pub const MAX_SANDBOX_ENTRIES: usize = 8;
/// The largest count: 2^53 - 1, the range a JSON number survives a round trip in (AG-UI's bound).
/// The sandbox's counts keep to it too.
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
    /// The sandbox names an environment that is not `kubernetes`, `local` or `devcontainer`.
    #[error("the sandbox names an environment the orchestrator does not know")]
    UnknownEnvironment,
    /// More than [`MAX_SANDBOX_ENTRIES`] sandbox entries.
    #[error("more than 8 sandbox entries")]
    TooManySandboxEntries,
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
            UsageInvalid::UnknownEnvironment => "unknown_environment",
            UsageInvalid::TooManySandboxEntries => "too_many_sandbox_entries",
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

/// Where a run's commands ran, as the `sandbox` member names it. Closed (ADR 0004): another word is
/// an invalid sandbox, and a new environment is a change of the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxEnvironment {
    /// A pod of the run in a Kubernetes cluster: the one environment that measures its commands.
    Kubernetes,
    /// The agent's own container.
    Local,
    /// A dev container of the repository.
    Devcontainer,
}

impl SandboxEnvironment {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            SandboxEnvironment::Kubernetes => "kubernetes",
            SandboxEnvironment::Local => "local",
            SandboxEnvironment::Devcontainer => "devcontainer",
        }
    }

    fn from_word(word: &str) -> Option<SandboxEnvironment> {
        match word {
            "kubernetes" => Some(SandboxEnvironment::Kubernetes),
            "local" => Some(SandboxEnvironment::Local),
            "devcontainer" => Some(SandboxEnvironment::Devcontainer),
            _ => None,
        }
    }
}

/// The commands a run ran in a sandbox of one shape: what the sandbox reserved and how long the
/// commands used it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxEntry {
    /// The CPU the sandbox reserved, in millicores. Absent when it reserved none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis: Option<u64>,
    /// The memory the sandbox reserved, in bytes. Absent when it reserved none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// How many commands ran.
    pub commands: u64,
    /// The sum of the commands' wall times, in milliseconds. Commands that ran at the same time are
    /// summed, so it can exceed the time the run took.
    pub milliseconds: u64,
}

/// The `sandbox` member of a task's totals (`usage-v1.md` section 5): where the run's commands ran
/// and, when that environment measures them, the time they used.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxUsage {
    /// Where the commands ran.
    pub environment: SandboxEnvironment,
    /// One entry per shape, at most [`MAX_SANDBOX_ENTRIES`]. **Absent is not measured, never zero**;
    /// an empty list is a sandbox that was asked for and ran no command, a measured zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Vec<SandboxEntry>>,
}

impl SandboxUsage {
    /// The sandbox `entry` (the value of the `sandbox` member): `{environment, usage?}`, at most
    /// [`MAX_SANDBOX_ENTRIES`] entries, each with `commands` and `milliseconds` and the optional
    /// `cpuMillis` and `memoryBytes`, every count a whole number from 0 to 2^53 - 1 (a whole double
    /// such as `10.0` counts). Unknown members are ignored.
    pub fn parse(entry: &Value) -> Result<SandboxUsage, UsageInvalid> {
        let entry = entry.as_object().ok_or(UsageInvalid::NotAnObject)?;
        let environment = match entry.get("environment") {
            None | Some(Value::Null) => return Err(UsageInvalid::Missing),
            Some(Value::String(word)) => {
                SandboxEnvironment::from_word(word).ok_or(UsageInvalid::UnknownEnvironment)?
            }
            Some(_) => return Err(UsageInvalid::WrongType),
        };
        let usage = match entry.get("usage") {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => {
                if items.len() > MAX_SANDBOX_ENTRIES {
                    return Err(UsageInvalid::TooManySandboxEntries);
                }
                let entries = items
                    .iter()
                    .map(|item| {
                        let item = item.as_object().ok_or(UsageInvalid::WrongType)?;
                        Ok(SandboxEntry {
                            cpu_millis: optional_count(item, "cpuMillis")?,
                            memory_bytes: optional_count(item, "memoryBytes")?,
                            commands: required_count(item, "commands")?,
                            milliseconds: required_count(item, "milliseconds")?,
                        })
                    })
                    .collect::<Result<Vec<_>, UsageInvalid>>()?;
                Some(entries)
            }
            Some(_) => return Err(UsageInvalid::WrongType),
        };
        let sandbox = SandboxUsage { environment, usage };
        sandbox.check()?;
        Ok(sandbox)
    }

    /// The rules of [`SandboxUsage::parse`] on a sandbox already built: the core's door.
    pub fn check(&self) -> Result<(), UsageInvalid> {
        let Some(entries) = &self.usage else {
            return Ok(());
        };
        if entries.len() > MAX_SANDBOX_ENTRIES {
            return Err(UsageInvalid::TooManySandboxEntries);
        }
        for e in entries {
            let counts = [
                e.cpu_millis,
                e.memory_bytes,
                Some(e.commands),
                Some(e.milliseconds),
            ];
            if counts.into_iter().flatten().any(|n| n > MAX_TOKEN_COUNT) {
                return Err(UsageInvalid::BadCount);
            }
        }
        Ok(())
    }
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
    /// The time the run's commands used in a sandbox, when the agent reported it and it passed the
    /// door. `None` is no sandbox member, or one that did not pass ([`Self::sandbox_rejected`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxUsage>,
    /// Why a `sandbox` member the agent sent was left out. The token totals beside it are still
    /// taken: the application counts the drop and logs the totals without a sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_rejected: Option<UsageInvalid>,
}

impl UsageTotals {
    /// The totals `entry` (the value under the extension's URI in a task's metadata) of the task
    /// `task`: `{totals: [...], sandbox?}`, at most [`MAX_USAGE_TOTALS`] entries, each a provider and
    /// model once. Members this does not know, of the object and of its entries, are ignored.
    ///
    /// The `sandbox` is read apart: one that breaks the contract does not make the totals invalid,
    /// it is left out and its reason kept in [`UsageTotals::sandbox_rejected`], so a fault in the
    /// newer member never loses the tokens that have always been reported.
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
        let (sandbox, sandbox_rejected) = match entry.get("sandbox") {
            None | Some(Value::Null) => (None, None),
            Some(value) => match SandboxUsage::parse(value) {
                Ok(sandbox) => (Some(sandbox), None),
                Err(why) => (None, Some(why)),
            },
        };
        let report = UsageTotals {
            task: task.to_owned(),
            totals,
            sandbox,
            sandbox_rejected,
        };
        report.check()?;
        Ok(report)
    }

    /// The rules of [`UsageTotals::parse`] on totals already built: the core's door. The `sandbox` is
    /// not part of it ([`SandboxUsage::check`] is): a bad one does not make the totals invalid.
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
    /// The time the run's commands used in a sandbox, when the agent reported it (`usage-v1.md`
    /// section 5). Absent: the agent did not say, or what it said did not pass the door. Events
    /// written before the member existed have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxUsage>,
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
/// again is dropped; a call report past [`MAX_USAGE_CALLS_PER_JOB`] is dropped and counted. A total's
/// `sandbox` that fails the door again is left out and the totals beside it are logged.
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
            // the sandbox is checked on its own: one that fails the door again is left out and the
            // totals are logged without it
            let sandbox = totals.sandbox.as_ref().filter(|s| s.check().is_ok());
            EventBody::ModelUsageTotal(ModelUsageTotalData {
                job: job.number,
                agent: actor.name.clone(),
                task: totals.task.clone(),
                path: ask_path().unwrap_or_default(),
                totals: totals.totals.clone(),
                sandbox: sandbox.cloned(),
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
    fn unknown_members_beside_the_totals_and_inside_their_entries_are_ignored() {
        // the contract's rule (`usage-v1.md`): a client reads the members it knows and ignores the
        // rest, so an agent can add a member without a new version of the extension
        let totals = UsageTotals::parse(
            "t1",
            &json!({"totals": [
                {"provider": "openai", "model": "glm-5.3", "inputTokens": 5, "outputTokens": 2,
                 "totalTokens": 7, "somethingNew": {"a": 1}}
            ], "somethingNew": [1, 2], "price": "3"}),
        )
        .unwrap();
        assert_eq!(totals.totals.len(), 1);
        assert_eq!(totals.totals[0].tokens.total_tokens, 7);
    }

    #[test]
    fn a_total_written_before_the_sandbox_member_reads_back() {
        // the stored `data` of a `model_usage_total` as the log holds it for every event written
        // before the member existed
        let data: ModelUsageTotalData = serde_json::from_value(json!({
            "job": 1, "agent": "coder", "task": "t1",
            "totals": [{"provider": "openai", "model": "m", "inputTokens": 5, "outputTokens": 2,
                        "totalTokens": 7}]
        }))
        .unwrap();
        assert_eq!(data.totals.len(), 1);
        assert_eq!(data.task, "t1");
    }

    fn with_sandbox(sandbox: Value) -> Value {
        json!({"totals": [
            {"provider": "openai", "model": "glm-5.3", "inputTokens": 5, "outputTokens": 2,
             "totalTokens": 7}
        ], "sandbox": sandbox})
    }

    fn entry(commands: u64, milliseconds: u64) -> SandboxEntry {
        SandboxEntry {
            cpu_millis: None,
            memory_bytes: None,
            commands,
            milliseconds,
        }
    }

    #[test]
    fn the_four_shapes_of_a_runs_sandbox_are_told_apart() {
        // measured: a pod, with the shape of its requests and what the commands used
        let measured = UsageTotals::parse(
            "t1",
            &with_sandbox(json!({"environment": "kubernetes", "usage": [
                {"cpuMillis": 250, "memoryBytes": 536_870_912_u64, "commands": 12, "milliseconds": 81234}]})),
        )
        .unwrap();
        assert_eq!(
            measured.sandbox,
            Some(SandboxUsage {
                environment: SandboxEnvironment::Kubernetes,
                usage: Some(vec![SandboxEntry {
                    cpu_millis: Some(250),
                    memory_bytes: Some(536_870_912),
                    commands: 12,
                    milliseconds: 81234,
                }]),
            })
        );
        assert_eq!(measured.sandbox_rejected, None);
        // a measured zero: a pod asked for and no command run
        let zero = UsageTotals::parse(
            "t1",
            &with_sandbox(json!({"environment": "kubernetes", "usage": []})),
        )
        .unwrap();
        assert_eq!(zero.sandbox.unwrap().usage, Some(vec![]));
        // not measured: the environment alone, which is not a zero
        for (word, environment) in [
            ("local", SandboxEnvironment::Local),
            ("devcontainer", SandboxEnvironment::Devcontainer),
        ] {
            let unmeasured =
                UsageTotals::parse("t1", &with_sandbox(json!({"environment": word}))).unwrap();
            assert_eq!(
                unmeasured.sandbox,
                Some(SandboxUsage {
                    environment,
                    usage: None
                })
            );
        }
        // the run asked for no environment: no sandbox, and nothing was rejected
        let none = UsageTotals::parse(
            "t1",
            &json!({"totals": [{"model": "m", "inputTokens": 1, "outputTokens": 1, "totalTokens": 2}]}),
        )
        .unwrap();
        assert_eq!((none.sandbox, none.sandbox_rejected), (None, None));
        // `null` is no member, as for the optional counts
        let null = UsageTotals::parse("t1", &with_sandbox(Value::Null)).unwrap();
        assert_eq!((null.sandbox, null.sandbox_rejected), (None, None));
        // and a run that made no model call still has its sandbox
        let only_sandbox = UsageTotals::parse(
            "t1",
            &json!({"totals": [], "sandbox": {"environment": "kubernetes", "usage": [
                {"commands": 1, "milliseconds": 5}]}}),
        )
        .unwrap();
        assert!(only_sandbox.totals.is_empty());
        assert_eq!(
            only_sandbox.sandbox.unwrap().usage,
            Some(vec![entry(1, 5)]),
            "the requests are optional: an entry with neither is a sandbox that reserved none"
        );
    }

    #[test]
    fn not_measured_and_a_measured_zero_stay_apart_on_the_wire() {
        let unmeasured = SandboxUsage {
            environment: SandboxEnvironment::Local,
            usage: None,
        };
        let zero = SandboxUsage {
            environment: SandboxEnvironment::Kubernetes,
            usage: Some(vec![]),
        };
        let wire = serde_json::to_value(&unmeasured).unwrap();
        assert_eq!(wire, json!({"environment": "local"}));
        assert_eq!(
            serde_json::from_value::<SandboxUsage>(wire).unwrap(),
            unmeasured
        );
        let wire = serde_json::to_value(&zero).unwrap();
        assert_eq!(wire, json!({"environment": "kubernetes", "usage": []}));
        assert_eq!(serde_json::from_value::<SandboxUsage>(wire).unwrap(), zero);
    }

    #[test]
    fn a_whole_number_written_as_a_double_counts_in_the_sandbox_too() {
        let totals = UsageTotals::parse(
            "t1",
            &with_sandbox(json!({"environment": "kubernetes", "usage": [
                {"cpuMillis": 250.0, "memoryBytes": 536_870_912.0, "commands": 12.0, "milliseconds": 81234.0}]})),
        )
        .unwrap();
        let sandbox = totals.sandbox.unwrap();
        assert_eq!(sandbox.usage.unwrap()[0].milliseconds, 81234);
    }

    #[test]
    fn a_sandbox_that_breaks_the_contract_is_left_out_and_the_tokens_beside_it_are_kept() {
        let nine: Vec<Value> = (0..9)
            .map(|i| json!({"cpuMillis": 100 + i, "commands": 1, "milliseconds": 1}))
            .collect();
        let eight: Vec<Value> = nine[..8].to_vec();
        let cases: Vec<(Value, UsageInvalid)> = vec![
            (json!("kubernetes"), UsageInvalid::NotAnObject),
            (json!({"usage": []}), UsageInvalid::Missing),
            (json!({"environment": 3}), UsageInvalid::WrongType),
            (
                json!({"environment": "gvisor"}),
                UsageInvalid::UnknownEnvironment,
            ),
            (
                json!({"environment": "Kubernetes"}),
                UsageInvalid::UnknownEnvironment,
            ),
            (
                json!({"environment": "kubernetes", "usage": {"commands": 1}}),
                UsageInvalid::WrongType,
            ),
            (
                json!({"environment": "kubernetes", "usage": ["12"]}),
                UsageInvalid::WrongType,
            ),
            (
                json!({"environment": "kubernetes", "usage": nine}),
                UsageInvalid::TooManySandboxEntries,
            ),
            (
                json!({"environment": "kubernetes", "usage": [{"milliseconds": 5}]}),
                UsageInvalid::Missing,
            ),
            (
                json!({"environment": "kubernetes", "usage": [{"commands": 5}]}),
                UsageInvalid::Missing,
            ),
            (
                json!({"environment": "kubernetes", "usage": [{"commands": 1.5, "milliseconds": 5}]}),
                UsageInvalid::BadCount,
            ),
            (
                json!({"environment": "kubernetes", "usage": [{"commands": 1, "milliseconds": -5}]}),
                UsageInvalid::BadCount,
            ),
            (
                json!({"environment": "kubernetes", "usage": [{"cpuMillis": 0.5, "commands": 1, "milliseconds": 5}]}),
                UsageInvalid::BadCount,
            ),
            (
                json!({"environment": "kubernetes", "usage": [{"memoryBytes": 9_007_199_254_740_992_u64, "commands": 1, "milliseconds": 5}]}),
                UsageInvalid::BadCount,
            ),
            (
                json!({"environment": "kubernetes", "usage": [{"commands": "1", "milliseconds": 5}]}),
                UsageInvalid::WrongType,
            ),
        ];
        for (sandbox, why) in cases {
            let totals = UsageTotals::parse("t1", &with_sandbox(sandbox.clone()))
                .unwrap_or_else(|e| panic!("the totals are not lost ({e}): {sandbox}"));
            assert_eq!(totals.sandbox, None, "{sandbox}");
            assert_eq!(totals.sandbox_rejected, Some(why), "{sandbox}");
            assert_eq!(totals.totals.len(), 1, "{sandbox}");
            assert_eq!(totals.totals[0].tokens.total_tokens, 7, "{sandbox}");
            assert_eq!(totals.check(), Ok(()), "{sandbox}");
        }
        // the limit itself is valid: eight entries
        let ok = UsageTotals::parse(
            "t1",
            &with_sandbox(json!({"environment": "kubernetes", "usage": eight})),
        )
        .unwrap();
        assert_eq!(ok.sandbox.unwrap().usage.unwrap().len(), 8);
        assert_eq!(ok.sandbox_rejected, None);
        // and a total that is itself broken is still invalid, whatever its sandbox says
        let mut broken = with_sandbox(json!({"environment": "local"}));
        broken["totals"][0]["totalTokens"] = json!(1);
        assert_eq!(
            UsageTotals::parse("t1", &broken),
            Err(UsageInvalid::TotalNotSum)
        );
    }

    #[test]
    fn unknown_members_of_the_sandbox_and_of_its_entries_are_ignored() {
        let totals = UsageTotals::parse(
            "t1",
            &with_sandbox(json!({
                "environment": "kubernetes", "node": "n1",
                "usage": [{"cpuMillis": 250, "commands": 2, "milliseconds": 10, "gpu": 1, "labels": {"a": "b"}}]
            })),
        )
        .unwrap();
        assert_eq!(totals.sandbox_rejected, None);
        assert_eq!(
            totals.sandbox,
            Some(SandboxUsage {
                environment: SandboxEnvironment::Kubernetes,
                usage: Some(vec![SandboxEntry {
                    cpu_millis: Some(250),
                    memory_bytes: None,
                    commands: 2,
                    milliseconds: 10
                }]),
            })
        );
    }

    #[test]
    fn a_built_sandbox_is_checked_again_where_the_core_records_it() {
        let too_many = SandboxUsage {
            environment: SandboxEnvironment::Kubernetes,
            usage: Some(vec![entry(1, 1); 9]),
        };
        assert_eq!(too_many.check(), Err(UsageInvalid::TooManySandboxEntries));
        let too_big = SandboxUsage {
            environment: SandboxEnvironment::Kubernetes,
            usage: Some(vec![entry(1, MAX_TOKEN_COUNT + 1)]),
        };
        assert_eq!(too_big.check(), Err(UsageInvalid::BadCount));
        let fine = SandboxUsage {
            environment: SandboxEnvironment::Kubernetes,
            usage: Some(vec![entry(MAX_TOKEN_COUNT, MAX_TOKEN_COUNT); 8]),
        };
        assert_eq!(fine.check(), Ok(()));
    }

    #[test]
    fn an_event_with_a_sandbox_reads_back_the_shape_it_was_written_in() {
        let data = ModelUsageTotalData {
            job: 1,
            agent: "coder".into(),
            task: "t1".into(),
            path: vec![],
            totals: vec![],
            sandbox: Some(SandboxUsage {
                environment: SandboxEnvironment::Kubernetes,
                usage: Some(vec![SandboxEntry {
                    cpu_millis: Some(250),
                    memory_bytes: Some(536_870_912),
                    commands: 12,
                    milliseconds: 81234,
                }]),
            }),
        };
        let wire = serde_json::to_value(&data).unwrap();
        assert_eq!(
            wire,
            json!({"job": 1, "agent": "coder", "task": "t1", "totals": [],
                   "sandbox": {"environment": "kubernetes", "usage": [
                       {"cpuMillis": 250, "memoryBytes": 536_870_912_u64, "commands": 12, "milliseconds": 81234}]}})
        );
        assert_eq!(
            serde_json::from_value::<ModelUsageTotalData>(wire).unwrap(),
            data
        );
        // and without one, the member is not written at all
        let bare = ModelUsageTotalData {
            sandbox: None,
            ..data
        };
        assert!(
            serde_json::to_value(&bare)
                .unwrap()
                .get("sandbox")
                .is_none()
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
