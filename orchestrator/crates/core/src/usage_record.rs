//! The usage record of a person (ADR 0065): what the `model_usage` and `model_usage_total` events of
//! a thread's log change in the ledger that outlives the thread.
//!
//! This module is pure. It says **what an event changes** ([`usage_change`]) and **what a row of the
//! ledger becomes** when it is applied ([`UsageRow::fold`]); a store applies the result inside the
//! transaction that appends the event, so the log and the ledger cannot disagree. It reads the
//! events as ADR 0056 logged them and adds no member to them.
//!
//! **One row per task.** A row is a task of an agent in a thread ([`UsageTaskKey`]), with the person
//! it belongs to, the UTC month it is counted in, the counts per provider and model and the
//! sandbox time of the task:
//!
//! * a `model_usage_total` **replaces** the row's counts and sandbox whole: the totals of a task are
//!   the record and cover every call it made so far, so a later total is never added to an earlier
//!   one (`usage-v1.md` section 4);
//! * a `model_usage` **adds** to the row's counts, but only while the task has no total: once a
//!   task has one, a call changes nothing, because the total covers every call of the task so far
//!   (a call logged after it is in it already, or belongs to the next turn, which the next total
//!   covers) and a call counted on top of a total could be counted twice;
//! * an event whose `seq` is not above the `seq` of the last event the row folded in changes nothing,
//!   so applying an event twice is applying it once;
//! * the row's month is the UTC month of the latest event that changed it.
//!
//! The counts of a row are capped at [`MAX_LEDGER_COUNT`], the largest `bigint`: a report is
//! checked to 2^53 - 1 ([`MAX_TOKEN_COUNT`](crate::MAX_TOKEN_COUNT)) and a job logs at most
//! [`MAX_USAGE_CALLS_PER_JOB`](crate::MAX_USAGE_CALLS_PER_JOB) calls, so only an agent that lies
//! on purpose gets there, and what it gets is a full counter, never a failed commit.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::event::{Event, EventBody};
use crate::ids::{ThreadId, UserId};
use crate::usage::{ModelTokens, SandboxEntry, SandboxEnvironment, SandboxUsage, TokenCounts};

/// The largest count a row of the ledger holds: `i64::MAX`, what a Postgres `bigint` holds.
pub const MAX_LEDGER_COUNT: u64 = i64::MAX as u64;

/// A UTC calendar month, written `YYYY-MM`: the bucket the ledger counts a task in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UsageMonth {
    year: i16,
    month: i8,
}

/// Text that is not a month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a month: expected YYYY-MM, a year from 0001 and a month from 01 to 12")]
pub struct UsageMonthError;

impl UsageMonth {
    /// The month `month` (1 to 12) of `year` (1 to 9999); `None` for anything else.
    pub fn new(year: i16, month: i8) -> Option<UsageMonth> {
        ((1..=9999).contains(&year) && (1..=12).contains(&month))
            .then_some(UsageMonth { year, month })
    }

    /// The UTC month `at` falls in. A time before year 1 counts in year 1 (no event has one).
    pub fn of(at: Timestamp) -> UsageMonth {
        let date = at.to_zoned(TimeZone::UTC).date();
        UsageMonth {
            year: date.year().clamp(1, 9999),
            month: date.month(),
        }
    }

    /// The year, 1 to 9999.
    pub const fn year(self) -> i16 {
        self.year
    }

    /// The month, 1 to 12.
    pub const fn month(self) -> i8 {
        self.month
    }
}

impl fmt::Display for UsageMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl FromStr for UsageMonth {
    type Err = UsageMonthError;

    /// Exactly `YYYY-MM`: four digits, a dash, two digits; nothing else (no sign, no space).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let b = s.as_bytes();
        if b.len() != 7 || b[4] != b'-' {
            return Err(UsageMonthError);
        }
        let digits = |range: std::ops::Range<usize>| -> Option<i16> {
            let part = &b[range];
            part.iter()
                .all(u8::is_ascii_digit)
                .then(|| part.iter().fold(0_i16, |n, d| n * 10 + i16::from(d - b'0')))
        };
        let year = digits(0..4).ok_or(UsageMonthError)?;
        let month = digits(5..7).ok_or(UsageMonthError)?;
        UsageMonth::new(year, i8::try_from(month).map_err(|_| UsageMonthError)?)
            .ok_or(UsageMonthError)
    }
}

impl From<UsageMonth> for String {
    fn from(month: UsageMonth) -> String {
        month.to_string()
    }
}

impl TryFrom<String> for UsageMonth {
    type Error = UsageMonthError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

/// Whose task a row is: the thread, the agent that ran it and the A2A task. The thread's agent and
/// an asked agent (ADR 0026) are apart by the agent; a task an agent continues under a later ask
/// is the same task, so it stays the same row, and its total replaces what was counted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UsageTaskKey {
    /// The thread the usage was logged in. Cleared (the row keeps no thread) when the thread is
    /// erased (ADR 0043).
    pub thread: ThreadId,
    /// The agent that made the calls: the thread's agent, or an asked agent.
    pub agent: String,
    /// The A2A task.
    pub task: String,
}

/// What a row of the ledger counts: the calls reported so far, or the task's latest total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageSource {
    /// The sum of the call reports logged, because the task has not said a total yet.
    Calls,
    /// The latest `model_usage_total` of the task.
    Total,
}

impl UsageSource {
    /// The spelling a store writes.
    pub const fn as_str(self) -> &'static str {
        match self {
            UsageSource::Calls => "calls",
            UsageSource::Total => "total",
        }
    }

    /// The source a store's text names.
    pub fn from_word(word: &str) -> Option<UsageSource> {
        match word {
            "calls" => Some(UsageSource::Calls),
            "total" => Some(UsageSource::Total),
            _ => None,
        }
    }
}

/// A task's row of the ledger, as a store keeps it. Counts only: no message, no thread content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRow {
    /// The person the thread belongs to.
    pub owner: UserId,
    /// Which task.
    pub key: UsageTaskKey,
    /// The month it is counted in: the month of the latest event that changed it.
    pub month: UsageMonth,
    /// Whether it counts calls or holds the task's total.
    pub source: UsageSource,
    /// The `seq` of the last event of the thread folded into the row.
    pub seq: i64,
    /// The counts, one entry per provider and model, ordered by provider (none first) and model.
    pub tokens: Vec<ModelTokens>,
    /// The sandbox time of the task, from its latest total; `None` for a task that has no total or
    /// whose total said none.
    pub sandbox: Option<SandboxUsage>,
}

/// What one usage event changes in the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageChange {
    /// The person the thread belongs to.
    pub owner: UserId,
    /// The task the event is about.
    pub key: UsageTaskKey,
    /// The event's `seq` in its thread.
    pub seq: i64,
    /// The UTC month of the event's time.
    pub month: UsageMonth,
    /// What the event says.
    pub what: UsageWhat,
}

/// What a usage event says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageWhat {
    /// One model call's counts (`model_usage`).
    Call(ModelTokens),
    /// A task's totals (`model_usage_total`).
    Total {
        /// One entry per provider and model.
        tokens: Vec<ModelTokens>,
        /// The sandbox time, when the agent reported one.
        sandbox: Option<SandboxUsage>,
    },
}

/// What `event`, an event of a thread of `owner`, changes in the ledger: `None` for every event that
/// is not a usage event.
pub fn usage_change(owner: &UserId, event: &Event) -> Option<UsageChange> {
    let (agent, task, what) = match &event.body {
        EventBody::ModelUsage(d) => (
            &d.agent,
            &d.task,
            UsageWhat::Call(ModelTokens {
                provider: d.provider.clone(),
                model: d.model.clone(),
                tokens: d.tokens,
            }),
        ),
        EventBody::ModelUsageTotal(d) => (
            &d.agent,
            &d.task,
            UsageWhat::Total {
                tokens: d.totals.clone(),
                sandbox: d.sandbox.clone(),
            },
        ),
        _ => return None,
    };
    Some(UsageChange {
        owner: owner.clone(),
        key: UsageTaskKey {
            thread: event.thread_id,
            agent: agent.clone(),
            task: task.clone(),
        },
        seq: event.seq,
        month: UsageMonth::of(event.at),
        what,
    })
}

impl UsageRow {
    /// The row after `change` is applied to `current` (the row of `change.key`, none if the task has
    /// none yet): `None` when the change leaves the row as it is.
    ///
    /// Never fails and never panics: a store applies it inside the commit of the event, and the
    /// ledger must not be a reason for a conversation to stop.
    pub fn fold(current: Option<&UsageRow>, change: &UsageChange) -> Option<UsageRow> {
        if current.is_some_and(|row| change.seq <= row.seq) {
            return None;
        }
        let (source, tokens, sandbox) = match &change.what {
            UsageWhat::Call(line) => {
                if current.is_some_and(|row| row.source == UsageSource::Total) {
                    return None;
                }
                let mut lines = current.map(|row| row.tokens.clone()).unwrap_or_default();
                add_line(&mut lines, line, MAX_LEDGER_COUNT);
                (UsageSource::Calls, lines, None)
            }
            UsageWhat::Total { tokens, sandbox } => {
                let mut lines = Vec::with_capacity(tokens.len());
                for line in tokens {
                    add_line(&mut lines, line, MAX_LEDGER_COUNT);
                }
                (UsageSource::Total, lines, sandbox.clone())
            }
        };
        Some(UsageRow {
            owner: change.owner.clone(),
            key: change.key.clone(),
            month: change.month,
            source,
            seq: change.seq,
            tokens: sorted(tokens),
            sandbox,
        })
    }
}

/// Adds `line` to the entry of its provider and model in `lines`, or appends it. Counts stop at
/// `limit`.
fn add_line(lines: &mut Vec<ModelTokens>, line: &ModelTokens, limit: u64) {
    if let Some(entry) = lines
        .iter_mut()
        .find(|e| e.provider == line.provider && e.model == line.model)
    {
        entry.tokens = plus(&entry.tokens, &line.tokens, limit);
    } else {
        lines.push(ModelTokens {
            provider: line.provider.clone(),
            model: line.model.clone(),
            tokens: plus(&TokenCounts::default(), &line.tokens, limit),
        });
    }
}

/// `a` and `b` added, every count stopping at `limit`. A part (reasoning, cached) is unknown until
/// one of the two says it, as in the projection of the same events.
fn plus(a: &TokenCounts, b: &TokenCounts, limit: u64) -> TokenCounts {
    let add = |x: u64, y: u64| x.saturating_add(y).min(limit);
    let part = |x: Option<u64>, y: Option<u64>| match (x, y) {
        (Some(x), Some(y)) => Some(add(x, y)),
        (Some(n), None) | (None, Some(n)) => Some(n.min(limit)),
        (None, None) => None,
    };
    TokenCounts {
        input_tokens: add(a.input_tokens, b.input_tokens),
        output_tokens: add(a.output_tokens, b.output_tokens),
        total_tokens: add(a.total_tokens, b.total_tokens),
        reasoning_tokens: part(a.reasoning_tokens, b.reasoning_tokens),
        cached_input_tokens: part(a.cached_input_tokens, b.cached_input_tokens),
        cache_write_input_tokens: part(a.cache_write_input_tokens, b.cache_write_input_tokens),
    }
}

/// `lines` ordered by provider (none first) and model: the order every store keeps and reads.
fn sorted(mut lines: Vec<ModelTokens>) -> Vec<ModelTokens> {
    lines.sort_by(|a, b| (a.provider.as_deref(), &a.model).cmp(&(b.provider.as_deref(), &b.model)));
    lines
}

/// The time the commands of tasks used in the sandboxes of one shape in a month, summed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxShape {
    /// Where the commands ran.
    pub environment: SandboxEnvironment,
    /// The CPU the sandbox reserved, in millicores; `None` when the entries said none.
    pub cpu_millis: Option<u64>,
    /// The memory the sandbox reserved, in bytes; `None` when the entries said none.
    pub memory_bytes: Option<u64>,
    /// How many commands ran.
    pub commands: u64,
    /// The sum of their wall times, in milliseconds.
    pub milliseconds: u64,
}

/// Tasks whose commands ran in an environment that does not measure them: their time is not zero,
/// it is not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmeasuredSandbox {
    /// The environment.
    pub environment: SandboxEnvironment,
    /// How many tasks of the month ran their commands there.
    pub tasks: u64,
}

/// What a person used in one month (the read side of the ledger): the sums PR 2's answer is made
/// of, before a price is put on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageMonthTotals {
    /// The month.
    pub month: UsageMonth,
    /// How many tasks the month holds.
    pub tasks: u64,
    /// The counts per provider and model, ordered by provider (none first) and model. A count past
    /// `u64::MAX` stops there.
    pub models: Vec<ModelTokens>,
    /// The measured sandbox time per environment and shape, ordered by environment word,
    /// millicores and bytes (none first).
    pub sandbox: Vec<SandboxShape>,
    /// The tasks whose sandbox was not measured, per environment, ordered by environment word.
    pub unmeasured: Vec<UnmeasuredSandbox>,
}

impl UsageMonthTotals {
    /// A month in which nothing was used.
    pub fn empty(month: UsageMonth) -> UsageMonthTotals {
        UsageMonthTotals {
            month,
            tasks: 0,
            models: Vec::new(),
            sandbox: Vec::new(),
            unmeasured: Vec::new(),
        }
    }

    /// Totals from sums a store computed, put in the order every store gives: how the Postgres
    /// adapter, which sums in SQL, and [`UsageMonthTotals::of`], which sums rows, agree.
    pub fn new(
        month: UsageMonth,
        tasks: u64,
        models: Vec<ModelTokens>,
        mut sandbox: Vec<SandboxShape>,
        mut unmeasured: Vec<UnmeasuredSandbox>,
    ) -> UsageMonthTotals {
        sandbox.sort_by(|a, b| {
            (a.environment.as_str(), a.cpu_millis, a.memory_bytes).cmp(&(
                b.environment.as_str(),
                b.cpu_millis,
                b.memory_bytes,
            ))
        });
        unmeasured.sort_by_key(|u| u.environment.as_str());
        UsageMonthTotals {
            month,
            tasks,
            models: sorted(models),
            sandbox,
            unmeasured,
        }
    }

    /// The totals of `rows`, which the caller has chosen: the rows of one person's month.
    pub fn of<'a>(
        month: UsageMonth,
        rows: impl IntoIterator<Item = &'a UsageRow>,
    ) -> UsageMonthTotals {
        let mut tasks = 0_u64;
        let mut models: Vec<ModelTokens> = Vec::new();
        let mut shapes: BTreeMap<(&'static str, Option<u64>, Option<u64>), SandboxShape> =
            BTreeMap::new();
        let mut unmeasured: BTreeMap<&'static str, UnmeasuredSandbox> = BTreeMap::new();
        for row in rows {
            tasks = tasks.saturating_add(1);
            for line in &row.tokens {
                add_line(&mut models, line, u64::MAX);
            }
            let Some(sandbox) = &row.sandbox else {
                continue;
            };
            let Some(entries) = &sandbox.usage else {
                let slot =
                    unmeasured
                        .entry(sandbox.environment.as_str())
                        .or_insert(UnmeasuredSandbox {
                            environment: sandbox.environment,
                            tasks: 0,
                        });
                slot.tasks = slot.tasks.saturating_add(1);
                continue;
            };
            for SandboxEntry {
                cpu_millis,
                memory_bytes,
                commands,
                milliseconds,
            } in entries
            {
                let shape = shapes
                    .entry((sandbox.environment.as_str(), *cpu_millis, *memory_bytes))
                    .or_insert(SandboxShape {
                        environment: sandbox.environment,
                        cpu_millis: *cpu_millis,
                        memory_bytes: *memory_bytes,
                        commands: 0,
                        milliseconds: 0,
                    });
                shape.commands = shape.commands.saturating_add(*commands);
                shape.milliseconds = shape.milliseconds.saturating_add(*milliseconds);
            }
        }
        UsageMonthTotals::new(
            month,
            tasks,
            models,
            shapes.into_values().collect(),
            unmeasured.into_values().collect(),
        )
    }
}
