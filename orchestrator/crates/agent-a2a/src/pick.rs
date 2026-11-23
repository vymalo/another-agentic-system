//! The model and reasoning effort of a message on the A2A client side (ADR 0086, `model-pick/v1`
//! and adam's `reasoning/v1`, ADR 0008): what the card says, and what the message carries.
//!
//! The card lists `model-pick/v1` with `params {choices: [{id, label}], default?}` and
//! `reasoning/v1` with `params {efforts, default?}` ([`pick_from_card`]); an extension counts only
//! when its URI is listed exactly **and** its params are well formed. A message that names a model
//! is sent with `{"model": "<id>"}` under the model-pick URI and a message that names an effort with
//! `{"effort": "<level>"}` under the reasoning URI, each URI activated in `A2A-Extensions` and
//! `message.extensions` for that message alone. A message that names nothing activates nothing and
//! carries no member.
//!
//! Unlike the locale, a pick is **never a hint**: a card that does not offer what the message names
//! is [`AgentError::Unsupported`] and nothing is sent ([`pick_metadata`]), because a person who
//! chose "deep" must not silently get the default.

use a2a::AgentCard;
use orch_core::{
    EffortOffer, Listing, MODEL_PICK_EXTENSION, ModelPickOffer, PickListings, REASONING_EXTENSION,
};
use orch_ports::{AgentError, SendRequest};
use serde_json::{Value, json};

/// What the card says under the two extensions of a pick.
pub fn pick_from_card(card: &AgentCard) -> PickListings {
    PickListings {
        model: listing(card, MODEL_PICK_EXTENSION, ModelPickOffer::from_params),
        effort: listing(card, REASONING_EXTENSION, EffortOffer::from_params),
    }
}

fn listing<T>(card: &AgentCard, uri: &str, read: impl Fn(&Value) -> Option<T>) -> Listing<T> {
    let Some(entry) = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == uri)
    else {
        return Listing::Absent;
    };
    entry
        .params
        .as_ref()
        .and_then(|params| {
            read(&Value::Object(
                params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ))
        })
        .map_or(Listing::Malformed, Listing::Offered)
}

/// The members of the message metadata that carry the pick: the model's (under
/// [`MODEL_PICK_EXTENSION`]) and the effort's (under [`REASONING_EXTENSION`]), each `None` when the
/// message names nothing on that axis.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct PickMetadata {
    pub(crate) model: Option<Value>,
    pub(crate) effort: Option<Value>,
}

/// The pick of `req` as message metadata, checked against the card read for this very call.
///
/// # Errors
/// [`AgentError::Unsupported`] when the message names a model or an effort the card does not offer
/// (or lists the extension not at all, or badly): nothing is sent, and the work is never retried
/// without the pick (ADR 0086 decision 6).
pub(crate) fn pick_metadata(
    req: &SendRequest,
    listings: &PickListings,
) -> Result<PickMetadata, AgentError> {
    let Some(pick) = req.pick.as_ref() else {
        return Ok(PickMetadata::default());
    };
    listings
        .check(pick)
        .map_err(|refusal| AgentError::Unsupported(refusal.to_string()))?;
    Ok(PickMetadata {
        model: pick
            .model
            .as_ref()
            .map(|id| json!({ "model": id.as_str() })),
        effort: pick
            .reasoning_effort
            .map(|level| json!({ "effort": level.as_str() })),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use a2a::{AgentCapabilities, AgentExtension};
    use orch_core::{AgentId, Classify, ErrorClass, ModelPick, ReasoningEffort};
    use orch_ports::{AgentEndpoint, SendContent};

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

    fn entry(uri: &str, params: Value) -> AgentExtension {
        AgentExtension {
            uri: uri.to_owned(),
            description: None,
            required: Some(false),
            params: match params {
                Value::Object(map) => Some(map.into_iter().collect::<HashMap<_, _>>()),
                _ => None,
            },
        }
    }

    fn full_card() -> AgentCard {
        card_with(vec![
            entry(
                MODEL_PICK_EXTENSION,
                json!({"choices": [{"id": "fast", "label": "Fast"}, {"id": "deep"}], "default": "fast"}),
            ),
            entry(
                REASONING_EXTENSION,
                json!({"efforts": ["low", "high"], "default": "low"}),
            ),
        ])
    }

    fn request(pick: Option<ModelPick>) -> SendRequest {
        SendRequest {
            endpoint: AgentEndpoint::a2a(AgentId::new("coder"), "http://coder/card", None),
            message_id: "m".to_owned(),
            context_id: None,
            task_id: None,
            reference_task_ids: Vec::new(),
            content: SendContent::Text("go".to_owned()),
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
            locale: None,
            pick,
        }
    }

    #[test]
    fn the_card_is_read_from_the_exact_uris_with_well_formed_params() {
        let listings = pick_from_card(&full_card());
        assert_eq!(listings.model.offer().unwrap().ids(), ["fast", "deep"]);
        assert_eq!(
            listings.effort.offer().unwrap().efforts,
            [ReasoningEffort::Low, ReasoningEffort::High]
        );
        assert_eq!(pick_from_card(&card_with(vec![])), PickListings::default());
        // a near miss of the URI is another extension
        let near = card_with(vec![entry(
            "https://agents.vymalo.com/a2a/extensions/model-pick/v2",
            json!({"choices": [{"id": "fast"}]}),
        )]);
        assert_eq!(pick_from_card(&near).model, Listing::Absent);
    }

    #[test]
    fn a_card_that_lists_it_badly_is_told_apart_from_one_that_does_not() {
        let card = card_with(vec![
            entry(MODEL_PICK_EXTENSION, json!({"choices": "fast"})),
            entry(REASONING_EXTENSION, json!({"efforts": ["extreme"]})),
        ]);
        let listings = pick_from_card(&card);
        assert_eq!(listings.model, Listing::Malformed);
        assert_eq!(listings.effort, Listing::Malformed);
    }

    #[test]
    fn a_message_that_names_no_pick_carries_no_member_whatever_the_card_says() {
        let listings = pick_from_card(&full_card());
        assert_eq!(
            pick_metadata(&request(None), &listings).unwrap(),
            PickMetadata::default()
        );
        // an empty pick is the same as none
        assert_eq!(
            pick_metadata(&request(None), &PickListings::default()).unwrap(),
            PickMetadata::default()
        );
    }

    #[test]
    fn a_pick_the_card_offers_arrives_in_the_metadata_axis_by_axis() {
        let listings = pick_from_card(&full_card());
        let both = ModelPick {
            model: Some(orch_core::ModelId::parse("deep").unwrap()),
            reasoning_effort: Some(ReasoningEffort::High),
        };
        assert_eq!(
            pick_metadata(&request(Some(both)), &listings).unwrap(),
            PickMetadata {
                model: Some(json!({"model": "deep"})),
                effort: Some(json!({"effort": "high"})),
            }
        );
        // an effort alone activates the reasoning extension and not the model's
        let effort = ModelPick::effort(ReasoningEffort::Low);
        assert_eq!(
            pick_metadata(&request(Some(effort)), &listings).unwrap(),
            PickMetadata {
                model: None,
                effort: Some(json!({"effort": "low"})),
            }
        );
    }

    #[test]
    fn a_pick_the_card_does_not_offer_is_refused_and_sends_nothing() {
        let listings = pick_from_card(&full_card());
        for pick in [
            ModelPick::model("huge").unwrap(),
            ModelPick::effort(ReasoningEffort::Max),
        ] {
            let error = pick_metadata(&request(Some(pick)), &listings).unwrap_err();
            assert!(matches!(error, AgentError::Unsupported(_)), "{error:?}");
            assert_eq!(error.class(), ErrorClass::Unsupported, "never retried");
        }
        // a card that lists neither refuses any pick
        let error = pick_metadata(
            &request(Some(ModelPick::model("fast").unwrap())),
            &PickListings::default(),
        )
        .unwrap_err();
        assert!(matches!(error, AgentError::Unsupported(_)));
    }
}
