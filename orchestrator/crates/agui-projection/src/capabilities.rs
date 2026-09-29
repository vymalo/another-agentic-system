//! The capabilities document of an agent (`GET /agui/agents/{agentId}/capabilities`), as a pure
//! function of what the live A2A card says.
//!
//! `docs/api/agui.md`, "Capabilities document". The spec fixes the shape (`AgentCapabilities`);
//! how a consumer retrieves it is ours. Everything in the card-derived members is optional: a
//! card that could not be read gives an honest, smaller document (ADR 0008: fail closed, read
//! live, never cached), and the transport members describe this orchestrator, not the agent.

use std::collections::BTreeMap;

use orch_agui_proto::{
    AgentCapabilities, HumanInTheLoopCapabilities, IdentityCapabilities, MultiAgentCapabilities,
    SubagentInfo, TransportCapabilities,
};
use orch_core::{AgentId, Releases};

use crate::vocab::RELEASE_CHANNELS_URI;

/// What the live card of an agent says, as far as the document needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardFacts {
    /// The card's description.
    pub description: Option<String>,
    /// The card's version.
    pub version: Option<String>,
    /// Present only when the card advertises the release-channels extension (ADR 0008).
    pub releases: Option<Releases>,
}

/// The `AgentCapabilities` of the agent `id` (display name `name`); `card` is `None` when the
/// card could not be read.
///
/// - `identity`: the display name, and the card's description and version when they are known;
/// - `transport`: streaming, and resumable by sequence number on the connect stream (a
///   transport of our own; the standard bindings do not resume);
/// - `humanInTheLoop`: interrupts (`RUN_FINISHED` with an interrupt outcome, answered by `resume`);
/// - `multiAgent`: the agent runs as a subagent of the run, under its own id as the name;
/// - `custom`: `[RELEASE_CHANNELS_URI]` when the card lists releases.
pub fn agent_capabilities(id: &AgentId, name: &str, card: Option<&CardFacts>) -> AgentCapabilities {
    let mut custom = BTreeMap::new();
    if let Some(releases) = card.and_then(|c| c.releases.as_ref())
        && let Ok(value) = serde_json::to_value(releases)
    {
        custom.insert(RELEASE_CHANNELS_URI.to_owned(), value);
    }
    let description = card.and_then(|c| c.description.clone());
    AgentCapabilities {
        identity: Some(IdentityCapabilities {
            name: Some(name.to_owned()),
            description: description.clone(),
            version: card.and_then(|c| c.version.clone()),
            ..IdentityCapabilities::default()
        }),
        transport: Some(TransportCapabilities {
            streaming: Some(true),
            resumable: Some(true),
            ..TransportCapabilities::default()
        }),
        multi_agent: Some(MultiAgentCapabilities {
            supported: Some(true),
            delegation: Some(true),
            subagents: Some(vec![SubagentInfo {
                name: id.to_string(),
                description,
            }]),
            ..MultiAgentCapabilities::default()
        }),
        human_in_the_loop: Some(HumanInTheLoopCapabilities {
            supported: Some(true),
            interrupts: Some(true),
        }),
        custom: (!custom.is_empty()).then_some(custom),
    }
}
