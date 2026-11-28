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
    EffortOffer, Listing, MODE_EXTENSION, MODEL_PICK_EXTENSION, ModeOffer, ModelPickOffer,
    PickListings, REASONING_EXTENSION,
};
use orch_ports::{AgentError, SendRequest};
use serde_json::{Value, json};

/// What the card says under the two extensions of a pick and `agent-mode/v1`.
pub fn pick_from_card(card: &AgentCard) -> PickListings {
    PickListings {
        model: listing(card, MODEL_PICK_EXTENSION, ModelPickOffer::from_params),
        effort: listing(card, REASONING_EXTENSION, EffortOffer::from_params),
        mode: listing(card, MODE_EXTENSION, ModeOffer::from_params),
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
/// [`MODEL_PICK_EXTENSION`]), the effort's (under [`REASONING_EXTENSION`]) and the mode's (under
/// [`MODE_EXTENSION`]), each `None` when the message names nothing on that axis.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct PickMetadata {
    pub(crate) model: Option<Value>,
    pub(crate) effort: Option<Value>,
    pub(crate) mode: Option<Value>,
}

/// The pick of `req` as message metadata, checked against the card read for this very call.
///
/// # Errors
/// [`AgentError::Unsupported`] when the message names a model, an effort or a mode the card does
/// not offer (or lists the extension not at all, or badly): nothing is sent, and the work is never
/// retried without the pick (ADR 0086, ADR 0109).
pub(crate) fn pick_metadata(
    req: &SendRequest,
    listings: &PickListings,
) -> Result<PickMetadata, AgentError> {
    let mut metadata = PickMetadata::default();
    if let Some(pick) = req.pick.as_ref() {
        listings
            .check(pick)
            .map_err(|refusal| AgentError::Unsupported(refusal.to_string()))?;
        metadata.model = pick
            .model
            .as_ref()
            .map(|id| json!({ "model": id.as_str() }));
        metadata.effort = pick
            .reasoning_effort
            .map(|level| json!({ "effort": level.as_str() }));
    }
    if let Some(mode) = req.mode.as_ref() {
        orch_core::check_mode(&listings.mode, Some(mode))
            .map_err(|refusal| AgentError::Unsupported(refusal.to_string()))?;
        metadata.mode = Some(json!({ "mode": mode.as_str() }));
    }
    Ok(metadata)
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
            mode: None,
            prompt_signature: None,
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
    fn a_choices_endpoint_is_read_from_the_live_card() {
        // The mock card of an agent that names the deployment's endpoint per model choice (ADR
        // 0084, the status note of 2026-11-24). adam's half that sends it is ANR-611.
        let card = card_with(vec![entry(
            MODEL_PICK_EXTENSION,
            json!({
                "choices": [
                    {"id": "fast", "label": "Fast", "endpoint": "frontier"},
                    {"id": "deep", "label": "Deep", "endpoint": "deep-endpoint"}
                ],
                "default": "fast"
            }),
        )]);
        let listings = pick_from_card(&card);
        let offer = listings.model.offer().expect("the card lists it");
        let deep = orch_core::ModelId::parse("deep").unwrap();
        assert_eq!(offer.endpoint_for(Some(&deep)), Some("deep-endpoint"));
        assert_eq!(offer.endpoint_for(None), Some("frontier"));
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
                mode: None,
            }
        );
        // an effort alone activates the reasoning extension and not the model's
        let effort = ModelPick::effort(ReasoningEffort::Low);
        assert_eq!(
            pick_metadata(&request(Some(effort)), &listings).unwrap(),
            PickMetadata {
                model: None,
                effort: Some(json!({"effort": "low"})),
                mode: None,
            }
        );
    }

    fn mode_card() -> AgentCard {
        card_with(vec![entry(
            orch_core::MODE_EXTENSION,
            json!({
                "modes": [{"name": "build", "title": "Build"}, {"name": "plan"}],
                "default": "build"
            }),
        )])
    }

    #[test]
    fn a_mode_the_card_offers_arrives_under_its_uri_and_one_it_does_not_is_refused() {
        let listings = pick_from_card(&mode_card());
        assert_eq!(listings.mode.offer().unwrap().names(), ["build", "plan"]);
        let plan = orch_core::ModeName::parse("plan").unwrap();
        let req = SendRequest {
            mode: Some(plan.clone()),
            ..request(None)
        };
        assert_eq!(
            pick_metadata(&req, &listings).unwrap(),
            PickMetadata {
                mode: Some(json!({"mode": "plan"})),
                ..PickMetadata::default()
            }
        );
        // a message that names no mode carries no member
        assert_eq!(
            pick_metadata(&request(None), &listings).unwrap(),
            PickMetadata::default()
        );
        // a mode the card does not offer, or a card that lists none, refuses and sends nothing
        for (req, listings) in [
            (
                SendRequest {
                    mode: Some(orch_core::ModeName::parse("review").unwrap()),
                    ..request(None)
                },
                listings.clone(),
            ),
            (
                SendRequest {
                    mode: Some(plan),
                    ..request(None)
                },
                PickListings::default(),
            ),
        ] {
            let error = pick_metadata(&req, &listings).unwrap_err();
            assert!(matches!(error, AgentError::Unsupported(_)), "{error:?}");
            assert_eq!(error.class(), ErrorClass::Unsupported, "never retried");
        }
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
