//! The A2UI extension on the A2A client side (ADR 0013, ADR 0008): reading it from a live card,
//! the capabilities the orchestrator tells an agent it renders, and the data part that carries a
//! user's action back.
//!
//! Detection is strict and fails closed: an extension entry counts only when its `uri` is exactly
//! one of the URIs of [`UiVersion::extension_uri`]. Both the current release (`v0.9.1`) and the
//! candidate (`v1.0`) are detected (open question 22); when a card lists both, `v0.9.1` is
//! spoken. Nothing is cached: the card is read for every call, so a card that gains or loses the
//! extension changes the next message.
//!
//! *Verified 2026-09-29* (<https://a2ui.org/specification/v0.9.1-a2ui-extension-specification/>,
//! <https://a2ui.org/specification/v1.0-a2ui-extension-specification/>):
//!
//! | | v0.9.1 | v1.0 |
//! |---|---|---|
//! | extension URI | `https://a2ui.org/a2a-extension/a2ui/v0.9.1` | `https://a2ui.org/a2a-extension/a2ui/v1.0` |
//! | `message.metadata` key | `a2uiClientCapabilities` | `a2uiRendererCapabilities` |
//! | value | `{"v0.9.1": {"supportedCatalogIds": [...]}}` | `{"v1.0": {"supportedCatalogIds": [...]}}` |
//!
//! Activation through the extension mechanism is optional; agents built on the A2UI SDK check it,
//! so the URI is also sent in the `A2A-Extensions` header and `message.extensions`.

use a2a::{AgentCard, Part};
use orch_core::{A2UI_MEDIA_TYPE, Timestamp, UiActionData, UiCatalogData, UiDelivery, UiVersion};
use orch_ports::UiSupport;
use serde_json::{Value, json};

/// The versions a card advertises, or `None` when it advertises none this build knows.
pub fn ui_from_card(card: &AgentCard) -> Option<UiSupport> {
    let listed: Vec<UiVersion> = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .filter_map(|e| UiVersion::from_extension_uri(&e.uri))
        .collect();
    // The current release first, whatever the order of the card.
    let versions: Vec<UiVersion> = [UiVersion::V0_9_1, UiVersion::V1_0]
        .into_iter()
        .filter(|v| listed.contains(v))
        .collect();
    let spoken = versions.first().copied()?;
    // The entry of the version we speak says whether the agent takes a catalog inline: only a
    // boolean `true` counts (A2UI: absent means false).
    let accepts_inline_catalogs = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .filter(|e| spoken.extension_uri() == Some(e.uri.as_str()))
        .any(|e| {
            e.params
                .as_ref()
                .and_then(|p| p.get("acceptsInlineCatalogs"))
                .and_then(Value::as_bool)
                == Some(true)
        });
    Some(UiSupport {
        versions,
        accepts_inline_catalogs,
    })
}

/// The catalog a delivery sends **inline**, in `inlineCatalogs` of the renderer's capabilities:
/// only an [`UiDelivery::Inline`] one, and only to an agent that takes one (`accepts_inline`,
/// [`UiSupport::accepts_inline_catalogs`]); to any other the agent is told which catalog is
/// current and asks for it again when it needs it (ADR 0023).
pub fn inline_catalog(
    delivery: Option<&UiDelivery>,
    accepts_inline: bool,
) -> Option<&UiCatalogData> {
    match delivery {
        Some(UiDelivery::Inline(data)) if accepts_inline => Some(data),
        Some(UiDelivery::Inline(_) | UiDelivery::Ref(_)) | None => None,
    }
}

/// The message metadata that tells the agent which catalogs the renderer supports, as `(key,
/// value)`, for the version spoken. `None` for a version that has no extension.
///
/// With a `delivery` (the card lists `ui-catalog/v1`, ADR 0023) our own catalog id is listed
/// **first** in `supportedCatalogIds`, in every such message, so that an agent that follows A2UI
/// strictly never takes the screen for one that lost the catalog; then the basic ids. The catalog
/// itself is in `inlineCatalogs` only when [`inline_catalog`] says so.
pub fn client_capabilities(
    version: UiVersion,
    delivery: Option<&UiDelivery>,
    accepts_inline: bool,
) -> Option<(&'static str, Value)> {
    let key = match version {
        UiVersion::V0_9_1 => "a2uiClientCapabilities",
        UiVersion::V1_0 => "a2uiRendererCapabilities",
        UiVersion::V0_9 => return None,
    };
    let ours = delivery.map(|d| d.reference().catalog_id);
    let mut ids: Vec<String> = ours.into_iter().collect();
    for id in version.basic_catalog_ids() {
        if !ids.iter().any(|known| known == id) {
            ids.push((*id).to_owned());
        }
    }
    let mut capabilities = json!({ "supportedCatalogIds": ids });
    if let Some(catalog) = inline_catalog(delivery, accepts_inline) {
        capabilities["inlineCatalogs"] = json!([catalog.catalog]);
    }
    Some((key, json!({ version.as_str(): capabilities })))
}

/// The value of the message's `metadata["https://agents.vymalo.com/a2a/extensions/ui-catalog/v1"]`
/// (ADR 0023): which catalog is current, and whether this message carries it in
/// `inlineCatalogs`. On every message of an agent whose card lists the extension, so the agent can
/// tell its copy is stale.
pub fn ui_catalog_metadata(delivery: &UiDelivery, inline: bool) -> Value {
    let current = delivery.reference();
    json!({
        "catalogId": current.catalog_id,
        "version": current.version,
        "digest": current.digest,
        "inline": inline,
    })
}

/// The data part that carries the user's action to the agent: one `action` message in an
/// `application/a2ui+json` part (the media type on the part, as A2A 1.0 has it, and the
/// extension's own `metadata.mimeType`).
pub fn action_part(action: &UiActionData, at: Timestamp) -> Part {
    let message = json!([{
        "version": action.version.as_str(),
        "action": {
            "name": action.name,
            "surfaceId": action.surface_id,
            "sourceComponentId": action.source_component_id,
            "timestamp": at.to_string(),
            "context": action.context,
        }
    }]);
    let mut part = Part::data(message).with_media_type(A2UI_MEDIA_TYPE);
    part.metadata = Some(std::collections::HashMap::from([(
        "mimeType".to_owned(),
        json!(A2UI_MEDIA_TYPE),
    )]));
    part
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use a2a::{AgentCapabilities, AgentExtension};
    use orch_core::{A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0, UiCatalogRef};

    use super::*;

    fn card_with(uris: &[&str]) -> AgentCard {
        let extensions = (!uris.is_empty()).then(|| {
            uris.iter()
                .map(|uri| AgentExtension {
                    uri: (*uri).to_owned(),
                    description: None,
                    required: None,
                    params: None,
                })
                .collect()
        });
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

    #[test]
    fn a_card_without_the_extension_has_no_ui_support() {
        assert_eq!(ui_from_card(&card_with(&[])), None);
        let other = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";
        assert_eq!(ui_from_card(&card_with(&[other])), None);
    }

    #[test]
    fn each_uri_is_detected_and_only_the_exact_uri() {
        let v091 = ui_from_card(&card_with(&[A2UI_EXTENSION_V0_9_1])).unwrap();
        assert_eq!(v091.versions, vec![UiVersion::V0_9_1]);
        let v10 = ui_from_card(&card_with(&[A2UI_EXTENSION_V1_0])).unwrap();
        assert_eq!(v10.versions, vec![UiVersion::V1_0]);
        for near in [
            "https://a2ui.org/a2a-extension/a2ui/v0.9",
            "https://a2ui.org/a2a-extension/a2ui/v0.9.1/",
            "http://a2ui.org/a2a-extension/a2ui/v0.9.1",
            "https://a2ui.org/a2a-extension/a2ui/v2.0",
            "https://a2ui.org/a2a-extension/a2ui/V1.0",
        ] {
            assert_eq!(ui_from_card(&card_with(&[near])), None, "{near}");
        }
    }

    #[test]
    fn both_uris_prefer_the_current_release_whatever_the_order() {
        for order in [
            [A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0],
            [A2UI_EXTENSION_V1_0, A2UI_EXTENSION_V0_9_1],
        ] {
            let s = ui_from_card(&card_with(&order)).unwrap();
            assert_eq!(s.versions, vec![UiVersion::V0_9_1, UiVersion::V1_0]);
            assert_eq!(s.preferred(), Some(UiVersion::V0_9_1));
        }
        // A repeated entry is one version.
        let s = ui_from_card(&card_with(&[A2UI_EXTENSION_V1_0, A2UI_EXTENSION_V1_0])).unwrap();
        assert_eq!(s.versions, vec![UiVersion::V1_0]);
    }

    #[test]
    fn capabilities_are_keyed_by_the_version_spoken() {
        let (key, value) = client_capabilities(UiVersion::V0_9_1, None, false).unwrap();
        assert_eq!(key, "a2uiClientCapabilities");
        assert_eq!(
            value,
            json!({"v0.9.1": {"supportedCatalogIds": [
                "https://a2ui.org/specification/v0_9_1/catalogs/basic/catalog.json",
                "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"]}})
        );
        let (key, value) = client_capabilities(UiVersion::V1_0, None, false).unwrap();
        assert_eq!(key, "a2uiRendererCapabilities");
        assert_eq!(
            value,
            json!({"v1.0": {"supportedCatalogIds": [
                "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json"]}})
        );
        assert_eq!(client_capabilities(UiVersion::V0_9, None, false), None);
        let one = Some(&UiDelivery::Ref(reference(1)));
        assert_eq!(client_capabilities(UiVersion::V0_9, one, true), None);
    }

    fn reference(version: u32) -> UiCatalogRef {
        UiCatalogRef {
            catalog_id: "https://agents.vymalo.com/a2ui/catalogs/chat".to_owned(),
            version,
            digest: format!("sha256:{}", "a".repeat(64)),
        }
    }

    fn inline(version: u32) -> UiDelivery {
        let catalog = json!({"catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat",
            "components": {"Note": {"type": "object"}}});
        UiDelivery::Inline(UiCatalogData {
            catalog_id: "https://agents.vymalo.com/a2ui/catalogs/chat".to_owned(),
            version,
            digest: format!("sha256:{}", "b".repeat(64)),
            catalog,
        })
    }

    fn card_with_params(uri: &str, params: Option<Value>) -> AgentCard {
        let mut card = card_with(&[uri]);
        let entry = card
            .capabilities
            .extensions
            .as_mut()
            .and_then(|e| e.first_mut())
            .unwrap();
        entry.params = params.map(|p| {
            p.as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        });
        card
    }

    #[test]
    fn a_card_says_whether_it_takes_a_catalog_inline_and_only_a_true_counts() {
        let takes = |params: Option<Value>| {
            ui_from_card(&card_with_params(A2UI_EXTENSION_V0_9_1, params))
                .unwrap()
                .accepts_inline_catalogs
        };
        assert!(takes(Some(json!({"acceptsInlineCatalogs": true}))));
        for no in [
            None,
            Some(json!({})),
            Some(json!({"supportedCatalogIds": ["x"]})),
            Some(json!({"acceptsInlineCatalogs": false})),
            Some(json!({"acceptsInlineCatalogs": "true"})),
            Some(json!({"acceptsInlineCatalogs": 1})),
        ] {
            assert!(!takes(no.clone()), "{no:?}");
        }
    }

    #[test]
    fn only_the_entry_of_the_version_we_speak_counts() {
        // v0.9.1 is spoken when the card lists both: v1.0's `true` is not for it
        let mut card = card_with(&[A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0]);
        let entries = card.capabilities.extensions.as_mut().unwrap();
        entries[1].params = Some(HashMap::from([(
            "acceptsInlineCatalogs".to_owned(),
            json!(true),
        )]));
        let ui = ui_from_card(&card).unwrap();
        assert_eq!(ui.preferred(), Some(UiVersion::V0_9_1));
        assert!(!ui.accepts_inline_catalogs);
        // and when it is the only entry, it is the one
        let only = card_with_params(
            A2UI_EXTENSION_V1_0,
            Some(json!({"acceptsInlineCatalogs": true})),
        );
        assert!(ui_from_card(&only).unwrap().accepts_inline_catalogs);
    }

    #[test]
    fn our_catalog_is_listed_first_in_every_message_and_the_catalog_rides_only_where_it_is_taken() {
        let basic = UiVersion::V0_9_1.basic_catalog_ids();
        let ours = "https://agents.vymalo.com/a2ui/catalogs/chat";
        let expected_ids = || {
            let mut ids = vec![ours];
            ids.extend(basic.iter().copied());
            ids
        };

        // inline, to an agent that takes it
        let delivery = inline(2);
        let (key, value) = client_capabilities(UiVersion::V0_9_1, Some(&delivery), true).unwrap();
        assert_eq!(key, "a2uiClientCapabilities");
        let UiDelivery::Inline(data) = &delivery else {
            unreachable!()
        };
        assert_eq!(
            value,
            json!({"v0.9.1": {"supportedCatalogIds": expected_ids(), "inlineCatalogs": [data.catalog]}})
        );
        // inline, to one that does not: the id is listed, the catalog is not sent
        let (_, value) = client_capabilities(UiVersion::V0_9_1, Some(&delivery), false).unwrap();
        assert_eq!(
            value,
            json!({"v0.9.1": {"supportedCatalogIds": expected_ids()}})
        );
        // a reference is never sent as a catalog
        let by_ref = UiDelivery::Ref(reference(2));
        let (_, value) = client_capabilities(UiVersion::V0_9_1, Some(&by_ref), true).unwrap();
        assert_eq!(
            value,
            json!({"v0.9.1": {"supportedCatalogIds": expected_ids()}})
        );
        // the other dialect, the same shape
        let (key, value) = client_capabilities(UiVersion::V1_0, Some(&delivery), true).unwrap();
        assert_eq!(key, "a2uiRendererCapabilities");
        let mut v1 = vec![ours];
        v1.extend(UiVersion::V1_0.basic_catalog_ids().iter().copied());
        assert_eq!(
            value,
            json!({"v1.0": {"supportedCatalogIds": v1, "inlineCatalogs": [data.catalog]}})
        );
        // no delivery: exactly what the message had before the catalog existed
        let (_, plain) = client_capabilities(UiVersion::V0_9_1, None, true).unwrap();
        assert_eq!(plain, json!({"v0.9.1": {"supportedCatalogIds": basic}}));
    }

    #[test]
    fn the_extension_metadata_names_the_current_catalog_and_says_whether_it_is_inline() {
        assert_eq!(
            ui_catalog_metadata(&inline(2), true),
            json!({"catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat", "version": 2,
                   "digest": format!("sha256:{}", "b".repeat(64)), "inline": true})
        );
        assert_eq!(
            ui_catalog_metadata(&UiDelivery::Ref(reference(3)), false),
            json!({"catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat", "version": 3,
                   "digest": format!("sha256:{}", "a".repeat(64)), "inline": false})
        );
        assert!(inline_catalog(Some(&inline(1)), true).is_some());
        assert!(inline_catalog(Some(&inline(1)), false).is_none());
        assert!(inline_catalog(Some(&UiDelivery::Ref(reference(1))), true).is_none());
        assert!(inline_catalog(None, true).is_none());
    }

    #[test]
    fn an_action_part_is_the_specs_action_message() {
        let mut context = serde_json::Map::new();
        context.insert("choice".into(), json!("a"));
        let action = UiActionData {
            surface_id: "s1".into(),
            name: "go".into(),
            source_component_id: "btn".into(),
            context,
            version: UiVersion::V0_9_1,
            run_id: Some("run-3".into()),
        };
        let at: Timestamp = "2026-09-29T12:00:00Z".parse().unwrap();
        let part = action_part(&action, at);
        assert_eq!(part.media_type.as_deref(), Some("application/a2ui+json"));
        assert_eq!(
            part.metadata,
            Some(HashMap::from([(
                "mimeType".to_owned(),
                json!("application/a2ui+json")
            )]))
        );
        let a2a::PartContent::Data(data) = &part.content else {
            panic!("not a data part");
        };
        assert_eq!(
            *data,
            json!([{
                "version": "v0.9.1",
                "action": {"name": "go", "surfaceId": "s1", "sourceComponentId": "btn",
                           "timestamp": "2026-09-29T12:00:00Z", "context": {"choice": "a"}}
            }]),
            "the run id is ours and is not sent"
        );
        // What the agent gets is what we would accept from an agent: a checked envelope.
        assert!(
            orch_core::check_operations(data).is_err(),
            "an action is not a surface op"
        );
    }
}
