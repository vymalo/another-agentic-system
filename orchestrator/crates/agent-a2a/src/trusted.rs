//! What a card says under `trusted-components/v1` (ADR 0083, `docs/api/trusted-components-v1.md`,
//! ADR 0008): the components an agent may ask the client to draw, read into a typed offer only
//! when the URI is listed exactly and its `params` are well formed.
//!
//! The extension has no send-side metadata of its own: the orchestrator activates it on a message
//! to an agent whose card offers it (so the agent knows the client can draw one), and the request
//! travels back in the agent's `Message` metadata, which `orch_a2a_mapping` reads.

use a2a::AgentCard;
use orch_core::{TRUSTED_COMPONENTS_EXTENSION, TrustedComponentsOffer};
use serde_json::Value;

/// The card's `trusted-components/v1` offer: `None` when the card does not list the URI exactly,
/// lists it with `params` that are not `{components: [...]}` as the contract has them, or names no
/// component this build can draw (fail closed).
pub fn trusted_components_from_card(card: &AgentCard) -> Option<TrustedComponentsOffer> {
    let entry = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == TRUSTED_COMPONENTS_EXTENSION)?;
    let params = entry.params.as_ref()?;
    TrustedComponentsOffer::from_params(&Value::Object(
        params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::HashMap;

    use a2a::{AgentCapabilities, AgentExtension};
    use orch_core::TrustedComponent;
    use serde_json::json;

    use super::*;

    fn card_with(extensions: Vec<AgentExtension>) -> AgentCard {
        AgentCard {
            name: "a".into(),
            description: "d".into(),
            version: "1".into(),
            supported_interfaces: vec![],
            capabilities: AgentCapabilities {
                extensions: Some(extensions),
                ..AgentCapabilities::default()
            },
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            provider: None,
            documentation_url: None,
            icon_url: None,
            security_schemes: None,
            security_requirements: None,
            signatures: None,
        }
    }

    fn entry(uri: &str, params: Option<Value>) -> AgentExtension {
        AgentExtension {
            uri: uri.to_owned(),
            description: None,
            required: Some(false),
            params: params.and_then(|p| match p {
                Value::Object(map) => Some(map.into_iter().collect::<HashMap<_, _>>()),
                _ => None,
            }),
        }
    }

    fn good() -> Value {
        json!({"components": ["McpCredential"]})
    }

    #[test]
    fn the_offer_is_read_from_the_exact_uri_with_well_formed_params() {
        let card = card_with(vec![
            entry(
                "https://agents.vymalo.com/a2a/extensions/access-tier/v1",
                None,
            ),
            entry(TRUSTED_COMPONENTS_EXTENSION, Some(good())),
        ]);
        let offer = trusted_components_from_card(&card).unwrap();
        assert_eq!(offer.components(), [TrustedComponent::McpCredential]);
        assert!(offer.offers(TrustedComponent::McpCredential));
    }

    #[test]
    fn the_adrs_card_lists_five_and_this_build_draws_the_one_it_has() {
        let card = card_with(vec![entry(
            TRUSTED_COMPONENTS_EXTENSION,
            Some(json!({"components": [
                "ApiKeyCreate", "McpCredential", "ModelServer", "SecretGenerate", "SecretCollect"
            ]})),
        )]);
        let offer = trusted_components_from_card(&card).unwrap();
        assert_eq!(offer.components(), [TrustedComponent::McpCredential]);
    }

    #[test]
    fn a_card_that_does_not_list_it_well_offers_nothing() {
        for (why, card) in [
            ("no extensions", card_with(vec![])),
            (
                "another version",
                card_with(vec![entry(
                    "https://agents.vymalo.com/a2a/extensions/trusted-components/v2",
                    Some(good()),
                )]),
            ),
            (
                "a trailing slash",
                card_with(vec![entry(
                    "https://agents.vymalo.com/a2a/extensions/trusted-components/v1/",
                    Some(good()),
                )]),
            ),
            (
                "no params",
                card_with(vec![entry(TRUSTED_COMPONENTS_EXTENSION, None)]),
            ),
            (
                "no components",
                card_with(vec![entry(TRUSTED_COMPONENTS_EXTENSION, Some(json!({})))]),
            ),
            (
                "a component this build cannot draw",
                card_with(vec![entry(
                    TRUSTED_COMPONENTS_EXTENSION,
                    Some(json!({"components": ["ApiKeyCreate"]})),
                )]),
            ),
            (
                // the first entry for the URI is the one read: a good one after it does not count
                "a bad entry before a good one",
                card_with(vec![
                    entry(
                        TRUSTED_COMPONENTS_EXTENSION,
                        Some(json!({"components": "x"})),
                    ),
                    entry(TRUSTED_COMPONENTS_EXTENSION, Some(good())),
                ]),
            ),
        ] {
            assert_eq!(trusted_components_from_card(&card), None, "{why}");
        }
    }
}
