//! The capabilities document of an agent (`GET /agui/agents/{agentId}/capabilities`), as a pure
//! function of what the live A2A card says.
//!
//! `docs/api/agui.md`, "Capabilities document". The spec fixes the shape (`AgentCapabilities`);
//! how a consumer retrieves it is ours. Everything in the card-derived members is optional: a
//! card that could not be read gives an honest, smaller document (ADR 0008: fail closed, read
//! live, never cached), and the transport members describe this orchestrator, not the agent.

use std::collections::{BTreeMap, BTreeSet};

use orch_agui_proto::{
    AgentCapabilities, HumanInTheLoopCapabilities, IdentityCapabilities, MultiAgentCapabilities,
    SubagentInfo, TransportCapabilities,
};
use orch_core::{
    ACCESS_TIER_EXTENSION, AccessTier, AgentId, KnownExtension, Releases, TierOffer, UiVersion,
};

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
    /// The A2UI extension versions the card advertises (ADR 0013); empty when it advertises none
    /// this build knows.
    pub ui: Vec<UiVersion>,
    /// The extensions of the orchestrator's own the card lists, by exact URI (`ui-catalog/v1`,
    /// `thread-tools/v1`, …: ADR 0008); empty when it lists none.
    pub extensions: BTreeSet<KnownExtension>,
    /// Present only when the card lists `access-tier/v1` with well-formed `params` (ADR 0061): the
    /// tiers the agent honours and its default.
    pub access_tier: Option<TierOffer>,
    /// Not the card's: the highest tier the person asking may use **of this agent**, over the
    /// roles that let them invoke it (`auth.roles.<role>.maxTier`), `None` when they have no cap
    /// on it. It goes in the document beside the card's offer, so that a screen draws a chip that
    /// offers what the message will be given.
    pub max_tier: Option<AccessTier>,
}

/// The `AgentCapabilities` of the agent `id` (display name `name`); `card` is `None` when the
/// card could not be read.
///
/// - `identity`: the display name, and the card's description and version when they are known;
/// - `transport`: streaming, and resumable by sequence number on the connect stream (a
///   transport of our own; the standard bindings do not resume);
/// - `humanInTheLoop`: interrupts (`RUN_FINISHED` with an interrupt outcome, answered by `resume`);
/// - `multiAgent`: the agent runs as a subagent of the run, under its own id as the name;
/// - `custom`: `[RELEASE_CHANNELS_URI]` when the card lists releases, and one key per A2UI
///   extension URI the card lists, `{supportedCatalogIds}` being the catalogs the web renders
///   (ADR 0013), and one key (an empty object) per extension of [`KnownExtension`] the card lists:
///   the key is the signal, so the web can flag an agent before it sends anything (an agent that
///   does not list `ui-catalog/v1` is sent no catalog), and `[ACCESS_TIER_EXTENSION]` with
///   `{tiers, default}` when the card lists `access-tier/v1` with well-formed params (ADR 0061: the
///   web draws its tier chip from it, and a card that lists it badly does not have the key), plus
///   `maxTier`, the highest tier the person asking may use of this agent, when their roles cap it. A card
///   that cannot be read declares none of them.
pub fn agent_capabilities(id: &AgentId, name: &str, card: Option<&CardFacts>) -> AgentCapabilities {
    let mut custom = BTreeMap::new();
    if let Some(releases) = card.and_then(|c| c.releases.as_ref())
        && let Ok(value) = serde_json::to_value(releases)
    {
        custom.insert(RELEASE_CHANNELS_URI.to_owned(), value);
    }
    for version in card.map(|c| c.ui.as_slice()).unwrap_or_default() {
        if let Some(uri) = version.extension_uri() {
            custom.insert(
                uri.to_owned(),
                serde_json::json!({ "supportedCatalogIds": version.basic_catalog_ids() }),
            );
        }
    }
    for extension in card.map(|c| &c.extensions).into_iter().flatten() {
        custom.insert(extension.uri().to_owned(), serde_json::json!({}));
    }
    if let Some(offer) = card.and_then(|c| c.access_tier.as_ref())
        && let Ok(mut value) = serde_json::to_value(offer)
    {
        // what the person asking may use of it, beside what the agent offers
        if let (Some(cap), Some(entry)) = (card.and_then(|c| c.max_tier), value.as_object_mut()) {
            entry.insert("maxTier".to_owned(), serde_json::json!(cap.as_str()));
        }
        custom.insert(ACCESS_TIER_EXTENSION.to_owned(), value);
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
