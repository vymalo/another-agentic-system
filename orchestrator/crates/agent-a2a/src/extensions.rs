//! The extensions of the orchestrator's own on a live card (ADR 0008): which of
//! [`KnownExtension`] an agent lists.
//!
//! Detection is strict and fails closed: an entry of `capabilities.extensions` counts only when its
//! `uri` is exactly one of the URIs of [`KnownExtension`]; a near miss (another version, a trailing
//! slash, another scheme or case) is not offered. Nothing is cached: the card is read for every
//! call, so a card that gains or loses an extension changes the next message. The release-channels
//! extension and A2UI have their own readings ([`releases_from_card`](crate::releases_from_card),
//! [`ui_from_card`](crate::ui_from_card)), because each carries parameters the orchestrator uses.

use std::collections::{BTreeMap, BTreeSet};

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

/// The parameters of a **build extension** the card lists, as text (ADR 0053): the first entry of
/// `capabilities.extensions` whose `uri` has a path segment named `build` (for example
/// `https://example.com/extensions/build/v1`), each scalar parameter (a string, a number or a
/// boolean) as `name -> text`. Empty for a card that lists none, and for one whose parameters
/// are not scalars. *Unverified (2026-10-07):* that adam-rs adds such an extension; this reads
/// whatever the card has and never fails on its absence. The text is the agent's own: the core
/// bounds it when it is recorded and it is never an instruction.
pub fn build_from_card(card: &AgentCard) -> BTreeMap<String, String> {
    let Some(entry) = card.capabilities.extensions.iter().flatten().find(|e| {
        e.uri
            .split(['/', '?', '#'])
            .any(|segment| segment.eq_ignore_ascii_case("build"))
    }) else {
        return BTreeMap::new();
    };
    entry
        .params
        .iter()
        .flatten()
        .filter_map(|(key, value)| {
            let text = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some((key.clone(), text))
        })
        .collect()
}

/// Whether the card lists `steps/v1` (ADR 0025): the agent can report its work as nested steps,
/// and the orchestrator activates the extension on a send and a resubscribe. Exact URI, read from
/// the live card, never remembered.
pub fn steps_from_card(card: &AgentCard) -> bool {
    extensions_from_card(card).contains(&KnownExtension::Steps)
}

/// Whether the card lists `text-stream/v1` (ADR 0027): the agent can stream its replies as it
/// writes them, and the orchestrator activates the extension on a send and a resubscribe. Exact
/// URI, read from the live card, never remembered.
pub fn text_stream_from_card(card: &AgentCard) -> bool {
    extensions_from_card(card).contains(&KnownExtension::TextStream)
}

/// Whether the card lists `usage/v1` (ADR 0056): the agent can report the tokens of each model call
/// and its task's totals, and the orchestrator activates the extension on a send and a resubscribe.
/// Exact URI, read from the live card, never remembered.
pub fn usage_from_card(card: &AgentCard) -> bool {
    extensions_from_card(card).contains(&KnownExtension::Usage)
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
    fn steps_are_detected_alone_by_their_exact_uri() {
        assert!(steps_from_card(&card_with(Some(vec![entry(
            KnownExtension::Steps.uri(),
            None
        )]))));
        assert!(!steps_from_card(&card_with(None)));
        assert!(!steps_from_card(&card_with(Some(vec![entry(
            KnownExtension::UiCatalog.uri(),
            None
        )]))));
        assert!(!steps_from_card(&card_with(Some(vec![entry(
            "https://agents.vymalo.com/a2a/extensions/steps/v2",
            None
        )]))));
    }

    #[test]
    fn text_streams_are_detected_alone_by_their_exact_uri() {
        assert!(text_stream_from_card(&card_with(Some(vec![entry(
            KnownExtension::TextStream.uri(),
            None
        )]))));
        assert!(!text_stream_from_card(&card_with(None)));
        assert!(!text_stream_from_card(&card_with(Some(vec![entry(
            KnownExtension::Steps.uri(),
            None
        )]))));
        for near in [
            "https://agents.vymalo.com/a2a/extensions/text-stream/v2",
            "https://agents.vymalo.com/a2a/extensions/text-stream/v1/",
            "https://agents.vymalo.com/a2a/extensions/Text-Stream/v1",
        ] {
            assert!(!text_stream_from_card(&card_with(Some(vec![entry(
                near, None
            )]))));
        }
    }

    #[test]
    fn usage_is_detected_alone_by_its_exact_uri() {
        assert!(usage_from_card(&card_with(Some(vec![entry(
            KnownExtension::Usage.uri(),
            Some(false)
        )]))));
        assert!(!usage_from_card(&card_with(None)));
        assert!(!usage_from_card(&card_with(Some(vec![entry(
            KnownExtension::Steps.uri(),
            None
        )]))));
        for near in [
            "https://agents.vymalo.com/a2a/extensions/usage/v2",
            "https://agents.vymalo.com/a2a/extensions/usage/v1/",
            "https://agents.vymalo.com/a2a/extensions/Usage/v1",
        ] {
            assert!(!usage_from_card(&card_with(Some(vec![entry(near, None)]))));
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
