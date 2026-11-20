//! What a card says under `child-sessions/v1` (ADR 0072, `docs/api/child-sessions-v1.md`, ADR
//! 0008): the helpers an agent runs as sessions, read into a typed offer only when the URI is listed
//! exactly and its `params` are well formed.

use a2a::AgentCard;
use orch_core::{CHILD_SESSIONS_EXTENSION, ChildSessionsOffer, MAX_CHILD_SESSIONS};
use orch_ports::SendContent;
use serde_json::{Value, json};

/// The card's `child-sessions/v1` offer: `None` when the card does not list the URI exactly, or
/// lists it with `params` that are not `{agents, maxOpen}` as the contract has them, or with a
/// `maxOpen` above [`MAX_CHILD_SESSIONS`] (fail closed).
pub fn child_sessions_from_card(card: &AgentCard) -> Option<ChildSessionsOffer> {
    let entry = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == CHILD_SESSIONS_EXTENSION)?;
    let params = entry.params.as_ref()?;
    ChildSessionsOffer::from_params(&Value::Object(
        params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ))
    // a parent notes at most MAX_CHILD_SESSIONS sessions in its life: an offer of more open at once
    // is one no listing could honour (and one that lets a listing pass the bound on listings)
    .filter(|offer| usize::try_from(offer.max_open()).is_ok_and(|n| n <= MAX_CHILD_SESSIONS))
}

/// What a person's start or close says under the URI in the message's `metadata` (contract sections
/// 4 and 6): `{start: {agent, description, keepOpen}}` or `{close: {cancel}}`; `None` for any other
/// content.
pub(crate) fn session_metadata(content: &SendContent) -> Option<Value> {
    match content {
        SendContent::SessionStart {
            agent,
            description,
            keep_open,
            ..
        } => Some(json!({"start": {
            "agent": agent, "description": description, "keepOpen": keep_open,
        }})),
        SendContent::SessionClose { cancel } => Some(json!({"close": {"cancel": cancel}})),
        SendContent::Text(_) | SendContent::UiAction { .. } => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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
    fn a_start_and_a_close_say_what_the_contract_has_and_a_message_says_nothing() {
        let start = SendContent::SessionStart {
            text: "go".into(),
            agent: "planner".into(),
            description: "Plan it".into(),
            keep_open: false,
        };
        assert_eq!(
            session_metadata(&start),
            Some(
                json!({"start": {"agent": "planner", "description": "Plan it", "keepOpen": false}})
            )
        );
        assert_eq!(
            session_metadata(&SendContent::SessionClose { cancel: true }),
            Some(json!({"close": {"cancel": true}}))
        );
        assert_eq!(session_metadata(&SendContent::Text("x".into())), None);
    }

    fn good() -> Value {
        json!({"agents": [{"name": "planner", "description": "Plans."}], "maxOpen": 4})
    }

    #[test]
    fn the_offer_is_read_from_the_exact_uri_with_well_formed_params() {
        let card = card_with(vec![
            entry("https://agents.vymalo.com/a2a/extensions/steer/v1", None),
            entry(CHILD_SESSIONS_EXTENSION, Some(good())),
        ]);
        let offer = child_sessions_from_card(&card).unwrap();
        assert_eq!(offer.max_open(), 4);
        assert!(offer.offers_agent("planner"));
    }

    #[test]
    fn a_card_that_does_not_list_it_well_offers_nothing() {
        for (why, card) in [
            ("no extensions", card_with(vec![])),
            (
                "another version",
                card_with(vec![entry(
                    "https://agents.vymalo.com/a2a/extensions/child-sessions/v2",
                    Some(good()),
                )]),
            ),
            (
                "a trailing slash",
                card_with(vec![entry(
                    "https://agents.vymalo.com/a2a/extensions/child-sessions/v1/",
                    Some(good()),
                )]),
            ),
            (
                "no params",
                card_with(vec![entry(CHILD_SESSIONS_EXTENSION, None)]),
            ),
            (
                "bad params",
                card_with(vec![entry(
                    CHILD_SESSIONS_EXTENSION,
                    Some(json!({"agents": [], "maxOpen": 4})),
                )]),
            ),
            (
                // the first entry for the URI is the one read: a good one after it does not count
                "a bad entry before a good one",
                card_with(vec![
                    entry(CHILD_SESSIONS_EXTENSION, Some(json!({"maxOpen": 4}))),
                    entry(CHILD_SESSIONS_EXTENSION, Some(good())),
                ]),
            ),
        ] {
            assert_eq!(child_sessions_from_card(&card), None, "{why}");
        }
    }
}
