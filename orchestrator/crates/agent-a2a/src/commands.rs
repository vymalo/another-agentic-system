//! The commands an agent serves, on the A2A client side (`commands/v1`, ADR 0079, ADR 0008): what
//! the live card says, and nothing else.
//!
//! The card lists the extension with `params: {commands: [...]}` ([`commands_from_card`]); the
//! orchestrator reads it only when the URI is listed exactly **and** the params are well formed. A
//! card with bad params offers nothing ([`CommandsListing::Malformed`]), so the capabilities
//! document carries no key and the web draws no `/` menu.
//!
//! The orchestrator sends no command: a command is the person's text (`/<name>` and its
//! arguments), inserted by the web and expanded by the agent. Nothing rides on a message for this
//! extension, so this module has no send-side half.

use a2a::AgentCard;
use orch_core::{COMMANDS_EXTENSION, CommandsListing, CommandsOffer};
use serde_json::Value;

/// What the card says under `commands/v1`: [`CommandsListing::Absent`] when it does not list the
/// URI exactly, [`CommandsListing::Malformed`] when it lists it with `params` that are not
/// `{commands: [...]}` as the contract has them, else the offer.
pub fn commands_from_card(card: &AgentCard) -> CommandsListing {
    let Some(entry) = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == COMMANDS_EXTENSION)
    else {
        return CommandsListing::Absent;
    };
    entry
        .params
        .as_ref()
        .and_then(|params| {
            CommandsOffer::from_params(&Value::Object(
                params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ))
        })
        .map_or(CommandsListing::Malformed, CommandsListing::Offered)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use a2a::{AgentCapabilities, AgentExtension};
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

    #[test]
    fn the_offer_is_read_from_the_exact_uri_with_well_formed_params() {
        let params = json!({"commands": [
            {"name": "review", "description": "Reviews a change.", "argumentHint": "<repo>"},
            {"name": "explain", "description": "Explains a file."}
        ]});
        let card = card_with(vec![entry(COMMANDS_EXTENSION, Some(params))]);
        let listing = commands_from_card(&card);
        let offer = listing.offer().unwrap();
        let names: Vec<_> = offer.commands().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["explain", "review"]);
        assert_eq!(
            offer.command("review").unwrap().argument_hint.as_deref(),
            Some("<repo>")
        );
    }

    #[test]
    fn a_card_that_does_not_list_the_exact_uri_lists_nothing() {
        let good = json!({"commands": [{"name": "review", "description": "d"}]});
        for card in [
            card_with(vec![]),
            // a near miss of the URI is another extension
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/commands/v2",
                Some(good.clone()),
            )]),
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/commands/v1/",
                Some(good.clone()),
            )]),
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/Commands/v1",
                Some(good.clone()),
            )]),
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/command/v1",
                Some(good),
            )]),
        ] {
            assert_eq!(commands_from_card(&card), CommandsListing::Absent);
        }
    }

    #[test]
    fn a_card_that_lists_it_badly_is_told_apart_from_one_that_does_not() {
        for params in [
            None,
            Some(json!({})),
            Some(json!({"commands": []})),
            Some(json!({"commands": "review"})),
            Some(json!({"commands": [{"name": "review"}]})),
            Some(json!({"commands": [{"name": "/review", "description": "d"}]})),
        ] {
            let card = card_with(vec![entry(COMMANDS_EXTENSION, params.clone())]);
            assert_eq!(
                commands_from_card(&card),
                CommandsListing::Malformed,
                "{params:?}"
            );
        }
    }

    #[test]
    fn a_card_that_carries_the_real_adam_shape_is_an_offer() {
        // adam-rs `ExtensionConfig::commands`: sorted by name, `argumentHint` only when it has one
        // (`crates/adam-a2a/src/extensions.rs`).
        let params = json!({"commands": [
            {"name": "explain", "description": "does explain"},
            {"name": "review", "description": "does review", "argumentHint": "<repo>"}
        ]});
        let card = card_with(vec![entry(COMMANDS_EXTENSION, Some(params))]);
        let offer = commands_from_card(&card).offer().cloned().unwrap();
        assert_eq!(
            serde_json::to_value(&offer).unwrap(),
            json!({"commands": [
                {"name": "explain", "description": "does explain"},
                {"name": "review", "description": "does review", "argumentHint": "<repo>"}
            ]})
        );
    }
}
