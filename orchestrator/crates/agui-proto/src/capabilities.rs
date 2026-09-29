//! `AgentCapabilities`: the typed snapshot of what an agent supports (schema `#AgentCapabilities`).
//!
//! Only the categories a producer of ours declares are typed here: identity, transport,
//! human-in-the-loop, multi-agent, and the open `custom` map. Every member is optional, and an
//! omitted member means "not declared", which is not "unsupported" (the schema says so), so a
//! producer states only what it knows. The spec does not define how a consumer retrieves the
//! document; where we serve it is `docs/api/agui.md`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::message::Metadata;

/// A snapshot of an agent's capabilities. All members are optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// Name, description, version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentityCapabilities>,
    /// Which transports the agent speaks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportCapabilities>,
    /// Multi-agent coordination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_agent: Option<MultiAgentCapabilities>,
    /// Human-in-the-loop support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_in_the_loop: Option<HumanInTheLoopCapabilities>,
    /// Integration-specific capabilities, open by key: a vendor prefix or a URI we own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<BTreeMap<String, Value>>,
}

/// Basic metadata about the agent (`identity`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityCapabilities {
    /// Human-readable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The framework or platform behind the agent.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// What the agent does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The agent's version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Who maintains it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Where its documentation is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    /// Integration-specific identity information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

/// Transport mechanisms (`transport`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportCapabilities {
    /// Responses stream over SSE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    /// Persistent WebSocket connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket: Option<bool>,
    /// The binary (protobuf over HTTP) binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_binary: Option<bool>,
    /// Webhooks after a run finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_notifications: Option<bool>,
    /// Interrupted streams can be resumed by sequence number, on a transport of the agent's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumable: Option<bool>,
}

/// A subagent the agent can invoke (`multiAgent.subagents[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentInfo {
    /// Unique name; the `name` of the `SUBAGENT_STARTED` event.
    pub name: String,
    /// What it specialises in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Multi-agent coordination (`multiAgent`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiAgentCapabilities {
    /// Takes part in any form of multi-agent coordination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported: Option<bool>,
    /// Delegates subtasks while keeping control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<bool>,
    /// Transfers the conversation to another agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoffs: Option<bool>,
    /// The subagents it can invoke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<SubagentInfo>>,
}

/// Human-in-the-loop support (`humanInTheLoop`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanInTheLoopCapabilities {
    /// Any form of human-in-the-loop interaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported: Option<bool>,
    /// Takes part in the interrupt protocol: a run ends with an interrupt outcome, and the
    /// answers come back in `RunAgentInput.resume`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupts: Option<bool>,
}
