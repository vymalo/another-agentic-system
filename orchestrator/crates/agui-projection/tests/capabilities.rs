//! The capabilities document of an agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;

use orch_agui_projection::{CardFacts, RELEASE_CHANNELS_URI, agent_capabilities};
use orch_agui_proto::testkit::assert_capabilities_conform;
use orch_core::{A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0, AgentId, Releases, UiVersion};
use serde_json::json;

fn releases() -> Releases {
    Releases {
        default_channel: "stable".to_owned(),
        channels: BTreeMap::from([
            ("stable".to_owned(), "r50".to_owned()),
            ("staging".to_owned(), "r51".to_owned()),
        ]),
        revisions: Some(vec!["r50".to_owned(), "r51".to_owned()]),
    }
}

#[test]
fn a_card_with_releases_is_declared_under_the_extension_uri() {
    let card = CardFacts {
        description: Some("Codes things".to_owned()),
        version: Some("2.1.0".to_owned()),
        releases: Some(releases()),
        ui: vec![],
    };
    let doc = agent_capabilities(&AgentId::new("coder"), "Coder", Some(&card));
    assert_capabilities_conform(&doc);
    assert_eq!(
        serde_json::to_value(&doc).unwrap(),
        json!({
            "identity": {"name": "Coder", "description": "Codes things", "version": "2.1.0"},
            "transport": {"streaming": true, "resumable": true},
            "multiAgent": {
                "supported": true,
                "delegation": true,
                "subagents": [{"name": "coder", "description": "Codes things"}]
            },
            "humanInTheLoop": {"supported": true, "interrupts": true},
            "custom": {
                RELEASE_CHANNELS_URI: {
                    "defaultChannel": "stable",
                    "channels": {"stable": "r50", "staging": "r51"},
                    "revisions": ["r50", "r51"]
                }
            }
        })
    );
}

#[test]
fn a_card_without_the_extension_declares_no_releases() {
    let card = CardFacts {
        description: None,
        version: None,
        releases: None,
        ui: vec![],
    };
    let doc = agent_capabilities(&AgentId::new("plain"), "Plain", Some(&card));
    assert_capabilities_conform(&doc);
    let json = serde_json::to_value(&doc).unwrap();
    assert!(json.get("custom").is_none(), "{json}");
    assert_eq!(json["identity"], json!({"name": "Plain"}));
    assert_eq!(json["multiAgent"]["subagents"], json!([{"name": "plain"}]));
}

#[test]
fn an_unreadable_card_gives_the_smaller_document() {
    // Fail closed (ADR 0008): nothing the card would have said is assumed.
    let doc = agent_capabilities(&AgentId::new("coder"), "Coder", None);
    assert_capabilities_conform(&doc);
    let json = serde_json::to_value(&doc).unwrap();
    assert!(json.get("custom").is_none());
    assert_eq!(json["identity"], json!({"name": "Coder"}));
    assert_eq!(
        json["transport"],
        json!({"streaming": true, "resumable": true})
    );
}

#[test]
fn a_card_with_a2ui_declares_the_catalogs_the_web_renders_under_each_uri() {
    for (versions, keys) in [
        (vec![UiVersion::V0_9_1], vec![A2UI_EXTENSION_V0_9_1]),
        (vec![UiVersion::V1_0], vec![A2UI_EXTENSION_V1_0]),
        (
            vec![UiVersion::V0_9_1, UiVersion::V1_0],
            vec![A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0],
        ),
    ] {
        let card = CardFacts {
            ui: versions,
            ..CardFacts::default()
        };
        let doc = agent_capabilities(&AgentId::new("ui"), "UI", Some(&card));
        assert_capabilities_conform(&doc);
        let json = serde_json::to_value(&doc).unwrap();
        let custom = json["custom"].as_object().unwrap();
        assert_eq!(custom.len(), keys.len(), "{json}");
        for key in keys {
            let ids = &custom[key]["supportedCatalogIds"];
            assert!(
                ids.as_array().is_some_and(|a| !a.is_empty()),
                "{key}: {ids}"
            );
        }
        assert!(custom.keys().all(|k| k != RELEASE_CHANNELS_URI));
    }
}

#[test]
fn an_unreadable_card_declares_no_a2ui() {
    let doc = agent_capabilities(&AgentId::new("ui"), "UI", None);
    assert!(serde_json::to_value(&doc).unwrap().get("custom").is_none());
}
