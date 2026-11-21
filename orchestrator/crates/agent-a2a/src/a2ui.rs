//! The A2UI extension on the A2A client side (ADR 0013, ADR 0008, ADR 0075): reading it from a live
//! card, the capabilities the orchestrator tells an agent it renders, and the data part that carries
//! a user's action back.
//!
//! **A2UI v1.0 only** (ADR 0075, a hard cutover): this build speaks one dialect, and a card that
//! lists another one is a card that does not speak A2UI to us. Detection is strict and fails
//! closed: an entry counts only when its `uri` is exactly
//! `https://a2ui.org/a2a-extension/a2ui/v1.0` ([`UiVersion::extension_uri`]). Nothing is cached: the
//! card is read for every call, so a card that gains or loses the extension changes the next
//! message.
//!
//! *Verified 2026-11-19* (`a2ui-project/a2ui` at `b4f91c2`,
//! `specification/v1_0/json/agent_to_renderer.json`, `renderer_to_agent.json` and
//! `renderer_capabilities.json`, and
//! <https://a2ui.org/specification/v1.0-a2ui-extension-specification/>):
//!
//! | | A2UI v1.0 |
//! |---|---|
//! | extension URI | `https://a2ui.org/a2a-extension/a2ui/v1.0` |
//! | `message.metadata` key | `a2uiRendererCapabilities` |
//! | value | `{"1.0": {"supportedCatalogIds": [...], "inlineCatalogs"?: [...]}}` |
//! | `version` of a message and of an action | `"1.0"` |
//!
//! Activation through the extension mechanism is optional; agents built on the A2UI SDK check it,
//! so the URI is also sent in the `A2A-Extensions` header and `message.extensions`.

use a2a::{AgentCard, Part};
use orch_core::{A2UI_MEDIA_TYPE, Timestamp, UiActionData, UiCatalogData, UiDelivery, UiVersion};
use orch_ports::UiSupport;
use serde_json::{Value, json};

/// The `message.metadata` key of the renderer's capabilities in A2UI v1.0 (ADR 0013, ADR 0075):
/// `{"1.0": {supportedCatalogIds, inlineCatalogs?}}`. `a2uiClientCapabilities`, the key of
/// `v0.9.1`, is never sent.
pub const RENDERER_CAPABILITIES_KEY: &str = "a2uiRendererCapabilities";

/// The versions a card advertises, or `None` when it advertises none this build knows.
///
/// A2UI v1.0 is the only dialect (ADR 0075): a card that lists `…/a2ui/v0.9.1` (or any other
/// near miss) speaks no A2UI this build can use, and is sent no capabilities.
pub fn ui_from_card(card: &AgentCard) -> Option<UiSupport> {
    let listed = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .filter_map(|e| UiVersion::from_extension_uri(&e.uri))
        .next()?;
    let versions = vec![listed];
    // The entry of the version we speak says whether the agent takes a catalog inline: only a
    // boolean `true` counts (A2UI: absent means false).
    let accepts_inline_catalogs = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .filter(|e| listed.extension_uri() == Some(e.uri.as_str()))
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
/// value)`: [`RENDERER_CAPABILITIES_KEY`], `{"1.0": {supportedCatalogIds, inlineCatalogs?}}`
/// (ADR 0013, ADR 0075).
///
/// With a `delivery` (the card lists `ui-catalog/v2`, ADR 0023) our own catalog id is listed
/// **first** in `supportedCatalogIds`, in every such message, so that an agent that follows A2UI
/// strictly never takes the screen for one that lost the catalog; then the basic catalog id of
/// v1.0. The catalog itself is in `inlineCatalogs` only when [`inline_catalog`] says so.
pub fn renderer_capabilities(
    delivery: Option<&UiDelivery>,
    accepts_inline: bool,
) -> (&'static str, Value) {
    let ours = delivery.map(|d| d.reference().catalog_id);
    let mut ids: Vec<String> = ours.into_iter().collect();
    for id in UiVersion::V1_0.basic_catalog_ids() {
        if !ids.iter().any(|known| known == id) {
            ids.push((*id).to_owned());
        }
    }
    let mut capabilities = json!({ "supportedCatalogIds": ids });
    if let Some(catalog) = inline_catalog(delivery, accepts_inline) {
        capabilities["inlineCatalogs"] = json!([catalog.catalog]);
    }
    (
        RENDERER_CAPABILITIES_KEY,
        json!({ UiVersion::V1_0.as_str(): capabilities }),
    )
}

/// The value of the message's `metadata["https://agents.vymalo.com/a2a/extensions/ui-catalog/v2"]`
/// (ADR 0023, ADR 0075): which catalog is current, and whether this message carries it in
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
///
/// **Exactly the five members** of an A2UI v1.0 action — `name`, `surfaceId`,
/// `sourceComponentId`, `timestamp` and `context` (ADR 0075, decision 4). A2UI v1.0 also defines
/// an optional `userMessage` and `metadata`; neither is ever forwarded: a line an agent wrote for a
/// click is never sent as the person's words (ADR 0013, rule 7).
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
    use a2a::{AgentCapabilities, AgentExtension};
    use orch_core::{A2UI_EXTENSION_V1_0, UiCatalogRef};

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

    /// ADR 0075: A2UI v1.0 only. A card that lists `v0.9.1` — the release this system spoke
    /// before the cutover — is a card that speaks no A2UI to this build, and neither is any near
    /// miss of the v1.0 URI.
    #[test]
    fn only_the_v1_0_uri_is_detected_and_a_near_miss_is_not() {
        let v10 = ui_from_card(&card_with(&[A2UI_EXTENSION_V1_0])).unwrap();
        assert_eq!(v10.versions, vec![UiVersion::V1_0]);
        for near in [
            "https://a2ui.org/a2a-extension/a2ui/v0.9",
            "https://a2ui.org/a2a-extension/a2ui/v0.9.1",
            "https://a2ui.org/a2a-extension/a2ui/v0.9.1/",
            "https://a2ui.org/a2a-extension/a2ui/v1.0/",
            "http://a2ui.org/a2a-extension/a2ui/v1.0",
            "https://a2ui.org/a2a-extension/a2ui/v2.0",
            "https://a2ui.org/a2a-extension/a2ui/V1.0",
        ] {
            assert_eq!(ui_from_card(&card_with(&[near])), None, "{near}");
        }
    }

    #[test]
    fn the_version_listed_twice_is_one_version_and_v0_9_1_beside_it_changes_nothing() {
        let twice = ui_from_card(&card_with(&[A2UI_EXTENSION_V1_0, A2UI_EXTENSION_V1_0])).unwrap();
        assert_eq!(twice.versions, vec![UiVersion::V1_0]);
        assert_eq!(twice.preferred(), Some(UiVersion::V1_0));
        // the retired URI beside it is not a second dialect: it is not detected at all
        let both = ui_from_card(&card_with(&[
            "https://a2ui.org/a2a-extension/a2ui/v0.9.1",
            A2UI_EXTENSION_V1_0,
        ]))
        .unwrap();
        assert_eq!(both.versions, vec![UiVersion::V1_0]);
    }

    #[test]
    fn the_capabilities_are_the_v1_0_renderer_capabilities_and_nothing_else_is_sent() {
        let (key, value) = renderer_capabilities(None, false);
        assert_eq!(key, "a2uiRendererCapabilities");
        assert_eq!(key, RENDERER_CAPABILITIES_KEY);
        assert_eq!(
            value,
            json!({"1.0": {"supportedCatalogIds": [
                "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json"]}})
        );
        // no `a2uiClientCapabilities`, no `"v0.9.1"`, no `a2uiRendererDataModel`: the web does not
        // implement data-model sync (ADR 0075)
        assert!(value.get("v0.9.1").is_none());
        assert!(value.get("v1.0").is_none());
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
            "protocolVersion": "1.0",
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
            ui_from_card(&card_with_params(A2UI_EXTENSION_V1_0, params))
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
    fn our_catalog_is_listed_first_in_every_message_and_the_catalog_rides_only_where_it_is_taken() {
        let basic = UiVersion::V1_0.basic_catalog_ids();
        let ours = "https://agents.vymalo.com/a2ui/catalogs/chat";
        let expected_ids = || {
            let mut ids = vec![ours];
            ids.extend(basic.iter().copied());
            ids
        };

        // inline, to an agent that takes it
        let delivery = inline(9);
        let (key, value) = renderer_capabilities(Some(&delivery), true);
        assert_eq!(key, "a2uiRendererCapabilities");
        let UiDelivery::Inline(data) = &delivery else {
            unreachable!()
        };
        assert_eq!(
            value,
            json!({"1.0": {"supportedCatalogIds": expected_ids(), "inlineCatalogs": [data.catalog]}})
        );
        // inline, to one that does not: the id is listed, the catalog is not sent
        let (_, value) = renderer_capabilities(Some(&delivery), false);
        assert_eq!(
            value,
            json!({"1.0": {"supportedCatalogIds": expected_ids()}})
        );
        // a reference is never sent as a catalog
        let by_ref = UiDelivery::Ref(reference(9));
        let (_, value) = renderer_capabilities(Some(&by_ref), true);
        assert_eq!(
            value,
            json!({"1.0": {"supportedCatalogIds": expected_ids()}})
        );
        // no delivery: exactly what the message had before the catalog existed
        let (_, plain) = renderer_capabilities(None, true);
        assert_eq!(plain, json!({"1.0": {"supportedCatalogIds": basic}}));
    }

    #[test]
    fn the_extension_metadata_names_the_current_catalog_and_says_whether_it_is_inline() {
        assert_eq!(
            ui_catalog_metadata(&inline(9), true),
            json!({"catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat", "version": 9,
                   "digest": format!("sha256:{}", "b".repeat(64)), "inline": true})
        );
        assert_eq!(
            ui_catalog_metadata(&UiDelivery::Ref(reference(9)), false),
            json!({"catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat", "version": 9,
                   "digest": format!("sha256:{}", "a".repeat(64)), "inline": false})
        );
        assert!(inline_catalog(Some(&inline(9)), true).is_some());
        assert!(inline_catalog(Some(&inline(9)), false).is_none());
        assert!(inline_catalog(Some(&UiDelivery::Ref(reference(9))), true).is_none());
        assert!(inline_catalog(None, true).is_none());
    }

    /// ADR 0075, decision 4: exactly the five members of a v1.0 action. `userMessage` and
    /// `metadata`, which A2UI v1.0 also allows, are never forwarded.
    #[test]
    fn an_action_part_is_the_specs_action_message_with_five_members() {
        let mut context = serde_json::Map::new();
        context.insert("choice".into(), json!("a"));
        let action = UiActionData {
            tier: None,
            tier_cap: None,
            surface_id: "s1".into(),
            name: "go".into(),
            source_component_id: "btn".into(),
            context,
            version: UiVersion::V1_0,
            run_id: Some("run-3".into()),
        };
        let at: Timestamp = "2026-11-19T12:00:00Z".parse().unwrap();
        let part = action_part(&action, at);
        assert_eq!(part.media_type.as_deref(), Some("application/a2ui+json"));
        assert_eq!(
            part.metadata,
            Some(std::collections::HashMap::from([(
                "mimeType".to_owned(),
                json!("application/a2ui+json")
            )]))
        );
        let a2a::PartContent::Data(data) = &part.content else {
            panic!("not a data part");
        };
        let sent = json!([{
            "version": "1.0",
            "action": {"name": "go", "surfaceId": "s1", "sourceComponentId": "btn",
                       "timestamp": "2026-11-19T12:00:00Z", "context": {"choice": "a"}}
        }]);
        assert_eq!(*data, sent, "the run id is ours and is not sent");
        assert_eq!(
            data[0]["action"].as_object().unwrap().len(),
            5,
            "exactly the five members: no userMessage, no metadata"
        );
        // What the agent gets is what we would accept from an agent: a checked envelope.
        assert!(
            orch_core::check_operations(data).is_err(),
            "an action is not a surface op"
        );
    }
}
