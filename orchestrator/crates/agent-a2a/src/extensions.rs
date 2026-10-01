//! The extensions of the orchestrator's own on a live card (ADR 0008): which of
//! [`KnownExtension`] an agent lists.
//!
//! Detection is strict and fails closed: an entry of `capabilities.extensions` counts only when its
//! `uri` is exactly one of the URIs of [`KnownExtension`]; a near miss (another version, a trailing
//! slash, another scheme or case) is not offered. Nothing is cached: the card is read for every
//! call, so a card that gains or loses an extension changes the next message. The release-channels
//! extension and A2UI have their own readings ([`releases_from_card`](crate::releases_from_card),
//! [`ui_from_card`](crate::ui_from_card)), because each carries parameters the orchestrator uses.

use std::collections::BTreeSet;

use a2a::AgentCard;
use orch_core::KnownExtension;

/// The [`KnownExtension`]s the card lists, each once whatever the order or the repeats of the card;
/// empty for a card that lists none.
pub fn extensions_from_card(card: &AgentCard) -> BTreeSet<KnownExtension> {
    card.capabilities
        .extensions
        .iter()
        .flatten()
        .filter_map(|e| KnownExtension::from_uri(&e.uri))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use a2a::{AgentCapabilities, AgentExtension};

    use super::*;

    fn card_with(extensions: Option<Vec<AgentExtension>>) -> AgentCard {
        AgentCard {
            name: "a".into(),
            description: "d".into(),
            version: "1".into(),
            supported_interfaces: vec![],
            capabilities: AgentCapabilities {
                extensions,
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

    fn entry(uri: &str, required: Option<bool>) -> AgentExtension {
        AgentExtension {
            uri: uri.to_owned(),
            description: None,
            required,
            params: None,
        }
    }

    #[test]
    fn a_card_without_extensions_offers_none() {
        assert!(extensions_from_card(&card_with(None)).is_empty());
        assert!(extensions_from_card(&card_with(Some(vec![]))).is_empty());
        let other = entry(
            "https://agents.vymalo.com/a2a/extensions/release-channels/v1",
            None,
        );
        assert!(extensions_from_card(&card_with(Some(vec![other]))).is_empty());
    }

    #[test]
    fn each_extension_is_detected_by_its_exact_uri() {
        for ext in KnownExtension::ALL {
            let got = extensions_from_card(&card_with(Some(vec![entry(ext.uri(), Some(false))])));
            assert_eq!(got, BTreeSet::from([ext]), "{ext}");
        }
    }

    #[test]
    fn a_near_miss_is_not_offered() {
        for near in [
            "https://agents.vymalo.com/a2a/extensions/ui-catalog/v2",
            "https://agents.vymalo.com/a2a/extensions/ui-catalog/v1/",
            "http://agents.vymalo.com/a2a/extensions/ui-catalog/v1",
            "https://agents.vymalo.com/a2a/extensions/UI-catalog/v1",
            "https://agents.vymalo.com/a2a/extensions/ui-catalog",
            "https://example.com/a2a/extensions/ui-catalog/v1",
            "ui-catalog",
        ] {
            assert!(
                extensions_from_card(&card_with(Some(vec![entry(near, None)]))).is_empty(),
                "{near}"
            );
        }
    }

    #[test]
    fn several_are_listed_once_in_any_order_whether_or_not_the_card_requires_them() {
        let forward: Vec<_> = KnownExtension::ALL
            .iter()
            .map(|e| entry(e.uri(), Some(true)))
            .collect();
        let mut backward = forward.clone();
        backward.reverse();
        backward.push(entry(KnownExtension::UiCatalog.uri(), None));
        let all = BTreeSet::from(KnownExtension::ALL);
        assert_eq!(extensions_from_card(&card_with(Some(forward))), all);
        assert_eq!(extensions_from_card(&card_with(Some(backward))), all);
    }
}
