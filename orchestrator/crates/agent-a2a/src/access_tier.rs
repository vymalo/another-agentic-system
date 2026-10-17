//! The access tier of a message on the A2A client side (`access-tier/v1`, ADR 0061, ADR 0008): what
//! the card says, and what the message carries.
//!
//! The card lists the extension with `params: {tiers, default}` ([`access_tier_from_card`]); the
//! orchestrator uses it only when the URI is listed exactly **and** the params are well formed. A
//! card with bad params offers no tier, so a message that names one is refused, and a person whose
//! role caps their tier is not run on such an agent (the dispatcher fails the job: it cannot be
//! given the cap). A message that names a tier is sent with
//! `{"tier": "<tier>"}` under the URI in its metadata, and the URI is activated in `A2A-Extensions`
//! and `message.extensions`, only when the card read for this very call honours that tier. Otherwise
//! **nothing is sent** and the call answers [`AgentError::Unsupported`]: a tier is never dropped, and
//! a message the person gave one never reaches an agent that would work under its default. A message
//! that names no tier does not activate the extension and is the plain message it always was.

use a2a::AgentCard;
use orch_core::{ACCESS_TIER_EXTENSION, TierListing, TierOffer};
use orch_ports::{AgentError, SendRequest};
use serde_json::{Value, json};

/// What the card says under `access-tier/v1`: [`TierListing::Absent`] when it does not list the URI
/// exactly, [`TierListing::Malformed`] when it lists it with `params` that are not `{tiers,
/// default}` as the contract has them, else the offer.
pub fn access_tier_from_card(card: &AgentCard) -> TierListing {
    let Some(entry) = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == ACCESS_TIER_EXTENSION)
    else {
        return TierListing::Absent;
    };
    entry
        .params
        .as_ref()
        .and_then(|params| {
            TierOffer::from_params(&Value::Object(
                params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ))
        })
        .map_or(TierListing::Malformed, TierListing::Offered)
}

/// Refuses a message that was sent plain because the card the sender read listed no tier, while the
/// sender holds the cap of the person's role ([`SendRequest::tier_cap`]), if the card read for this
/// very call lists the extension by now. The agent was redeployed between the two reads; sending
/// would run the message at the agent's own default, above the cap. The error is retryable, so the
/// sender decides again from the card as it stands. A message that holds no cap, or that names a
/// tier, is not this check's.
///
/// # Errors
/// [`AgentError::Protocol`] (retryable) as above: the caller sends nothing.
pub(crate) fn check_plain_under_cap(
    req: &SendRequest,
    listing: &TierListing,
) -> Result<(), AgentError> {
    if req.tier.is_none() && req.tier_cap.is_some() && *listing != TierListing::Absent {
        return Err(AgentError::protocol(
            "the agent's access tiers changed while the message was being sent; it is decided again",
        ));
    }
    Ok(())
}

/// The value of the extension's key in the message metadata, or `None` when the message gets none
/// because it names no tier.
///
/// # Errors
/// [`AgentError::Unsupported`] when the message names a tier and the card (`offer`) does not honour
/// it any more: the caller sends nothing.
pub(crate) fn tier_metadata(
    req: &SendRequest,
    offer: Option<&TierOffer>,
) -> Result<Option<Value>, AgentError> {
    let Some(tier) = req.tier else {
        return Ok(None);
    };
    if offer.is_some_and(|offer| offer.honours(tier)) {
        Ok(Some(json!({ "tier": tier })))
    } else {
        Err(AgentError::Unsupported(format!(
            "the agent no longer takes the access tier '{tier}'"
        )))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use a2a::{AgentCapabilities, AgentExtension};
    use orch_core::{AccessTier, AgentId, Classify};
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

    fn request(tier: Option<AccessTier>) -> SendRequest {
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
            tier,
            tier_cap: None,
        }
    }

    #[test]
    fn the_offer_is_read_from_the_exact_uri_with_well_formed_params() {
        let params = json!({"tiers": ["plan", "edits", "auto", "bypass"], "default": "auto"});
        let card = card_with(vec![entry(ACCESS_TIER_EXTENSION, Some(params))]);
        let listing = access_tier_from_card(&card);
        let offer = listing.offer().unwrap();
        assert_eq!(offer.default_tier(), AccessTier::Auto);
        assert!(offer.honours(AccessTier::Bypass));
    }

    #[test]
    fn a_card_that_does_not_list_the_exact_uri_lists_nothing() {
        let good = json!({"tiers": ["plan"], "default": "plan"});
        for card in [
            card_with(vec![]),
            // a near miss of the URI is another extension
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/access-tier/v2",
                Some(good.clone()),
            )]),
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/access-tier/v1/",
                Some(good.clone()),
            )]),
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/Access-Tier/v1",
                Some(good),
            )]),
        ] {
            assert_eq!(access_tier_from_card(&card), TierListing::Absent);
        }
    }

    fn plain(cap: Option<AccessTier>, tier: Option<AccessTier>) -> SendRequest {
        SendRequest {
            endpoint: AgentEndpoint::a2a(AgentId::new("a"), "http://a/card", None),
            message_id: "m".into(),
            context_id: None,
            task_id: None,
            reference_task_ids: Vec::new(),
            content: SendContent::Text("hi".into()),
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
            mentions: Vec::new(),
            tier,
            tier_cap: cap,
        }
    }

    #[test]
    fn a_plain_message_under_a_cap_is_refused_once_the_card_lists_the_extension() {
        let offered = TierListing::Offered(
            TierOffer::from_params(&json!({"tiers": ["plan", "auto"], "default": "auto"})).unwrap(),
        );
        for listing in [offered, TierListing::Malformed] {
            let refused =
                check_plain_under_cap(&plain(Some(AccessTier::Edits), None), &listing).unwrap_err();
            assert!(refused.is_retryable(), "{refused:?}");
        }
        // the card still lists nothing: the plain message is the right one
        check_plain_under_cap(&plain(Some(AccessTier::Edits), None), &TierListing::Absent).unwrap();
        // no cap held, or a tier named: not this check's
        let offered = TierListing::Offered(
            TierOffer::from_params(&json!({"tiers": ["plan"], "default": "plan"})).unwrap(),
        );
        check_plain_under_cap(&plain(None, None), &offered).unwrap();
        check_plain_under_cap(
            &plain(Some(AccessTier::Edits), Some(AccessTier::Plan)),
            &offered,
        )
        .unwrap();
    }

    #[test]
    fn a_card_that_lists_it_badly_is_told_apart_from_one_that_does_not() {
        for params in [
            None,
            Some(json!({})),
            Some(json!({"tiers": ["plan"], "default": "bypass"})),
            Some(json!({"tiers": ["plan", "yolo"], "default": "plan"})),
            Some(json!({"tiers": [], "default": "plan"})),
        ] {
            let card = card_with(vec![entry(ACCESS_TIER_EXTENSION, params.clone())]);
            assert_eq!(
                access_tier_from_card(&card),
                TierListing::Malformed,
                "{params:?}"
            );
        }
    }

    #[test]
    fn a_message_that_names_no_tier_gets_no_metadata_whatever_the_card_says() {
        let offer = TierOffer::from_params(&json!({"tiers": ["plan"], "default": "plan"}));
        assert_eq!(tier_metadata(&request(None), offer.as_ref()).unwrap(), None);
        assert_eq!(tier_metadata(&request(None), None).unwrap(), None);
    }

    #[test]
    fn a_tier_the_card_honours_is_the_message_metadata() {
        let offer =
            TierOffer::from_params(&json!({"tiers": ["plan", "edits"], "default": "edits"}));
        assert_eq!(
            tier_metadata(&request(Some(AccessTier::Plan)), offer.as_ref()).unwrap(),
            Some(json!({"tier": "plan"}))
        );
    }

    #[test]
    fn a_tier_the_card_does_not_honour_sends_nothing() {
        let offer = TierOffer::from_params(&json!({"tiers": ["plan"], "default": "plan"}));
        for offer in [offer.as_ref(), None] {
            let error = tier_metadata(&request(Some(AccessTier::Bypass)), offer).unwrap_err();
            assert!(matches!(error, AgentError::Unsupported(_)));
            assert_eq!(
                error.public_detail(),
                "the agent no longer takes the access tier 'bypass'"
            );
        }
    }
}
