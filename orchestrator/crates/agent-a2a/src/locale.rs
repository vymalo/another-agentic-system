//! The interface locale of a message on the A2A client side (`locale/v1`, ADR 0085, ADR 0008): what
//! the card says, and what the message carries.
//!
//! The card lists the extension, with `params: {locale}` when it says which language it writes in by
//! itself ([`locale_from_card`]); the orchestrator uses it only when the URI is listed exactly
//! **and** `params` are well formed. A card that lists it badly offers nothing, and a message to it
//! is sent plain. A message that names a locale is sent with `{"locale": "<tag>"}` under the URI in
//! its metadata, and the URI is activated in `A2A-Extensions` and `message.extensions`, only when
//! the card read for this very call lists it well formed.
//!
//! Unlike a tier, a locale is a **hint**: a card that does not list the extension, or lists it
//! badly, gets the plain A2A message the agent has always taken, and the person's message is never
//! held back because a language cannot be delivered. A message that names no locale does not
//! activate the extension and carries no member at all.

use a2a::AgentCard;
use orch_core::{LOCALE_EXTENSION, LocaleListing, LocaleOffer};
use orch_ports::SendRequest;
use serde_json::{Value, json};

/// What the card says under `locale/v1`: [`LocaleListing::Absent`] when it does not list the URI
/// exactly, [`LocaleListing::Malformed`] when it lists it with a `locale` member that is not a
/// BCP 47 tag, else the offer.
pub fn locale_from_card(card: &AgentCard) -> LocaleListing {
    let Some(entry) = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == LOCALE_EXTENSION)
    else {
        return LocaleListing::Absent;
    };
    entry
        .params
        .as_ref()
        .and_then(|params| {
            LocaleOffer::from_params(&Value::Object(
                params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ))
        })
        .map_or(LocaleListing::Malformed, LocaleListing::Offered)
}

/// The value of the extension's key in the message metadata, or `None` when the message carries
/// none: it names no locale, or the card read for this call does not list the extension well formed
/// (a hint that cannot be delivered is dropped, never a reason to send nothing).
pub(crate) fn locale_metadata(req: &SendRequest, listing: &LocaleListing) -> Option<Value> {
    let hint = req.locale.as_ref().filter(|_| listing.activates())?;
    Some(json!({ "locale": hint.as_str() }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use a2a::{AgentCapabilities, AgentExtension};
    use orch_core::{AgentId, LocaleHint};
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

    fn request(locale: Option<&str>) -> SendRequest {
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
            locale: locale.map(|tag| LocaleHint::parse(tag).unwrap()),
        }
    }

    #[test]
    fn the_offer_is_read_from_the_exact_uri_with_well_formed_params() {
        let card = card_with(vec![entry(
            LOCALE_EXTENSION,
            Some(json!({"locale": "en-GB"})),
        )]);
        let listing = locale_from_card(&card);
        let offer = listing.offer().unwrap();
        assert_eq!(
            offer.default_locale().map(LocaleHint::as_str),
            Some("en-GB")
        );
        // a card that lists it and says nothing of its own language still takes a hint
        let card = card_with(vec![entry(LOCALE_EXTENSION, Some(json!({})))]);
        assert!(locale_from_card(&card).activates());
    }

    #[test]
    fn a_card_that_does_not_list_the_exact_uri_lists_nothing() {
        let good = json!({"locale": "en-GB"});
        for card in [
            card_with(vec![]),
            // a near miss of the URI is another extension
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/locale/v2",
                Some(good.clone()),
            )]),
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/locale/v1/",
                Some(good.clone()),
            )]),
            card_with(vec![entry(
                "https://agents.vymalo.com/a2a/extensions/Locale/v1",
                Some(good),
            )]),
        ] {
            assert_eq!(locale_from_card(&card), LocaleListing::Absent);
        }
    }

    #[test]
    fn a_card_that_lists_it_badly_is_told_apart_from_one_that_does_not() {
        for params in [
            None,
            Some(json!({"locale": "en_CM"})),
            Some(json!({"locale": 3})),
            Some(json!({"locale": ""})),
        ] {
            let card = card_with(vec![entry(LOCALE_EXTENSION, params.clone())]);
            assert_eq!(
                locale_from_card(&card),
                LocaleListing::Malformed,
                "{params:?}"
            );
        }
    }

    #[test]
    fn a_message_that_names_no_locale_carries_no_member_whatever_the_card_says() {
        let listing = LocaleListing::Offered(LocaleOffer::from_params(&json!({})).unwrap());
        assert_eq!(locale_metadata(&request(None), &listing), None);
        assert_eq!(
            locale_metadata(&request(None), &LocaleListing::Absent),
            None
        );
    }

    #[test]
    fn a_locale_goes_to_an_agent_that_lists_the_extension() {
        let listing = LocaleListing::Offered(LocaleOffer::from_params(&json!({})).unwrap());
        assert_eq!(
            locale_metadata(&request(Some("fr-FR")), &listing),
            Some(json!({"locale": "fr-FR"}))
        );
    }

    #[test]
    fn a_card_without_the_extension_gets_the_plain_message() {
        for listing in [LocaleListing::Absent, LocaleListing::Malformed] {
            assert_eq!(locale_metadata(&request(Some("fr-FR")), &listing), None);
        }
    }
}
