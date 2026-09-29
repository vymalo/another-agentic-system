//! Run outcomes, interrupts and resume entries, token usage.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{InterruptId, SubagentRunId, ToolCallId};
use crate::message::Metadata;

/// Something a run needs from outside before it can continue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interrupt {
    /// Identifies the interrupt; a [`ResumeEntry`] answers it by this id.
    pub id: InterruptId,
    /// Why the run stopped (open string, for example `input_required`).
    pub reason: String,
    /// A human-readable prompt for whoever answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The tool call this concerns, for a tool approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<ToolCallId>,
    /// A JSON Schema describing the expected answer, carried opaquely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Metadata>,
    /// When the interrupt stops being answerable. ISO 8601 by convention; the schema does not
    /// constrain it, so neither do we.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// The subagent invocation this belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<SubagentRunId>,
}

impl Interrupt {
    /// An interrupt with only the required members.
    pub fn new(id: impl Into<InterruptId>, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            reason: reason.into(),
            message: None,
            tool_call_id: None,
            response_schema: None,
            expires_at: None,
            metadata: None,
            subagent_run_id: None,
        }
    }
}

/// How a run ended when it did not fail (`RUN_FINISHED.outcome`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum RunFinishedOutcome {
    /// The run completed. Equivalent to an absent outcome.
    Success {
        /// Frontend tool calls the run left unanswered, in call order.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pending_tool_call_ids: Option<Vec<ToolCallId>>,
    },
    /// The run is paused until the interrupts are answered by a later run's `resume`.
    Interrupt {
        /// What the run is waiting for. The schema requires at least one.
        interrupts: Vec<Interrupt>,
    },
    /// The run was stopped on purpose. Neither success nor failure.
    Cancelled,
}

impl RunFinishedOutcome {
    /// `success` with nothing pending.
    pub fn success() -> Self {
        Self::Success {
            pending_tool_call_ids: None,
        }
    }
}

/// How a subagent invocation's segment ended (`SUBAGENT_FINISHED.outcome`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum SubagentFinishedOutcome {
    /// The work completed. Equivalent to an absent outcome.
    Success,
    /// The invocation waits for outside input and may reappear in a later run.
    Suspended {
        /// The interrupts it waits on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interrupt_ids: Option<Vec<InterruptId>>,
    },
}

/// Whether a resumed interrupt was answered or abandoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResumeStatus {
    /// Answered; `payload` carries the answer.
    Resolved,
    /// Abandoned.
    Cancelled,
}

/// An answer to one interrupt, sent on the run that continues from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeEntry {
    /// The interrupt being answered.
    pub interrupt_id: InterruptId,
    /// Answered or abandoned.
    pub status: ResumeStatus,
    /// The answer. Any non-null JSON value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// Envelope information about the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

/// Token counts for one provider and model.
///
/// Counts are bounded by 2^53 - 1 in the schema (the range a JSON number round-trips in);
/// the type does not enforce that, [`testkit`](crate::testkit) does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// Which provider served the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Which model served the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Every prompt token (a total).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Every completion token (a total).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Input plus output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// A part of the output total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// A part of the input total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    /// A part of the input total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
}
