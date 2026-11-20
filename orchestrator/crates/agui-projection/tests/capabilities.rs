//! The capabilities document of an agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{BTreeMap, BTreeSet};

use orch_agui_projection::{CardFacts, RELEASE_CHANNELS_URI, agent_capabilities};
use orch_agui_proto::testkit::assert_capabilities_conform;
use orch_core::{
    A2UI_EXTENSION_V1_0, ACCESS_TIER_EXTENSION, AgentId, CHILD_SESSIONS_EXTENSION,
    COMMANDS_EXTENSION, ChildSessionsOffer, CommandsOffer, KnownExtension, Releases, TierOffer,
    UiVersion,
};
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
        extensions: BTreeSet::new(),
        access_tier: None,
        max_tier: None,
        commands: None,
        child_sessions: None,
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
        extensions: BTreeSet::new(),
        access_tier: None,
        max_tier: None,
        commands: None,
        child_sessions: None,
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
fn a_card_with_a2ui_declares_the_catalogs_the_web_renders_under_the_v1_0_uri() {
    // one dialect (ADR 0075): the key is the v1.0 URI, and the catalogs are the ones the web
    // renders under it
    let card = CardFacts {
        ui: vec![UiVersion::V1_0],
        ..CardFacts::default()
    };
    let doc = agent_capabilities(&AgentId::new("ui"), "UI", Some(&card));
    assert_capabilities_conform(&doc);
    let json = serde_json::to_value(&doc).unwrap();
    let custom = json["custom"].as_object().unwrap();
    assert_eq!(custom.len(), 1, "{json}");
    assert_eq!(
        custom[A2UI_EXTENSION_V1_0]["supportedCatalogIds"],
        json!(UiVersion::V1_0.basic_catalog_ids()),
        "the one basic catalog of v1.0"
    );
    assert!(custom.keys().all(|k| k != RELEASE_CHANNELS_URI));
}

#[test]
fn an_unreadable_card_declares_no_a2ui() {
    let doc = agent_capabilities(&AgentId::new("ui"), "UI", None);
    assert!(serde_json::to_value(&doc).unwrap().get("custom").is_none());
}

#[test]
fn a_card_that_lists_our_extensions_declares_each_under_its_uri() {
    for listed in [
        vec![],
        vec![KnownExtension::UiCatalog],
        vec![KnownExtension::ThreadTools, KnownExtension::Mentions],
        KnownExtension::ALL.to_vec(),
    ] {
        let card = CardFacts {
            extensions: listed.iter().copied().collect(),
            ..CardFacts::default()
        };
        let doc = agent_capabilities(&AgentId::new("x"), "X", Some(&card));
        assert_capabilities_conform(&doc);
        let json = serde_json::to_value(&doc).unwrap();
        if listed.is_empty() {
            assert!(json.get("custom").is_none(), "{json}");
            continue;
        }
        let custom = json["custom"].as_object().unwrap();
        assert_eq!(custom.len(), listed.len(), "{json}");
        for ext in listed {
            // the key is the signal: there is nothing more to say about these
            assert_eq!(custom[ext.uri()], json!({}), "{ext}");
        }
    }
}

#[test]
fn our_extensions_come_beside_the_a2ui_ones_and_the_releases() {
    let card = CardFacts {
        releases: Some(releases()),
        ui: vec![UiVersion::V1_0],
        extensions: BTreeSet::from([KnownExtension::UiCatalog]),
        ..CardFacts::default()
    };
    let doc = agent_capabilities(&AgentId::new("x"), "X", Some(&card));
    assert_capabilities_conform(&doc);
    let json = serde_json::to_value(&doc).unwrap();
    let mut keys: Vec<&str> = json["custom"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut want = vec![
        RELEASE_CHANNELS_URI,
        A2UI_EXTENSION_V1_0,
        KnownExtension::UiCatalog.uri(),
    ];
    want.sort_unstable();
    assert_eq!(keys, want);
}

#[test]
fn an_unreadable_card_declares_none_of_our_extensions() {
    let doc = agent_capabilities(&AgentId::new("x"), "X", None);
    assert!(serde_json::to_value(&doc).unwrap().get("custom").is_none());
}

#[test]
fn a_card_that_lists_the_access_tier_declares_its_tiers_and_default_under_the_uri() {
    // the web draws its tier chip from this key (ADR 0061)
    let offer = TierOffer::from_params(&json!({
        "tiers": ["bypass", "plan", "edits"],
        "default": "edits",
    }))
    .unwrap();
    let card = CardFacts {
        access_tier: Some(offer),
        ..CardFacts::default()
    };
    let doc = agent_capabilities(&AgentId::new("coder"), "Coder", Some(&card));
    assert_capabilities_conform(&doc);
    let json = serde_json::to_value(&doc).unwrap();
    assert_eq!(
        json["custom"],
        json!({ACCESS_TIER_EXTENSION: {"tiers": ["plan", "edits", "bypass"], "default": "edits"}}),
        "lowest first, the card's default"
    );
}

#[test]
fn the_cap_of_the_person_asking_rides_in_the_entry_beside_the_agents_offer() {
    let offer = || {
        TierOffer::from_params(&json!({"tiers": ["plan", "edits", "auto"], "default": "auto"}))
            .unwrap()
    };
    let capped = CardFacts {
        access_tier: Some(offer()),
        max_tier: Some(orch_core::AccessTier::Edits),
        ..CardFacts::default()
    };
    let doc = agent_capabilities(&AgentId::new("coder"), "Coder", Some(&capped));
    assert_capabilities_conform(&doc);
    assert_eq!(
        serde_json::to_value(&doc).unwrap()["custom"][ACCESS_TIER_EXTENSION],
        json!({"tiers": ["plan", "edits", "auto"], "default": "auto", "maxTier": "edits"})
    );
    // a cap on an agent that lists no tier says nothing: there is no chip to limit
    let nothing = CardFacts {
        max_tier: Some(orch_core::AccessTier::Edits),
        ..CardFacts::default()
    };
    let doc = agent_capabilities(&AgentId::new("coder"), "Coder", Some(&nothing));
    assert!(
        serde_json::to_value(&doc)
            .unwrap()
            .get("custom")
            .is_none_or(|c| c.get(ACCESS_TIER_EXTENSION).is_none())
    );
}

#[test]
fn a_card_that_lists_no_access_tier_or_an_unreadable_one_has_no_key_for_it() {
    for card in [None, Some(CardFacts::default())] {
        let doc = agent_capabilities(&AgentId::new("x"), "X", card.as_ref());
        let json = serde_json::to_value(&doc).unwrap();
        assert!(
            json.get("custom")
                .is_none_or(|c| c.get(ACCESS_TIER_EXTENSION).is_none()),
            "{json}"
        );
    }
}

#[test]
fn a_card_that_lists_child_sessions_declares_its_agents_and_limit_under_the_uri() {
    // the web offers "New subagent" from this key (ADR 0072)
    let offer = ChildSessionsOffer::from_params(&json!({
        "agents": [{"name": "planner", "description": "Plans."}],
        "maxOpen": 16,
    }))
    .unwrap();
    let card = CardFacts {
        child_sessions: Some(offer),
        ..CardFacts::default()
    };
    let doc = agent_capabilities(&AgentId::new("chat"), "Chat", Some(&card));
    assert_capabilities_conform(&doc);
    assert_eq!(
        serde_json::to_value(&doc).unwrap()["custom"],
        json!({CHILD_SESSIONS_EXTENSION: {
            "agents": [{"name": "planner", "description": "Plans."}],
            "maxOpen": 16
        }})
    );
    // the smallest offer is declared too
    let one = CardFacts {
        child_sessions: ChildSessionsOffer::from_params(&json!({
            "agents": [{"name": "planner", "description": ""}],
            "maxOpen": 1,
        })),
        ..CardFacts::default()
    };
    let doc = agent_capabilities(&AgentId::new("chat"), "Chat", Some(&one));
    assert_eq!(
        serde_json::to_value(&doc).unwrap()["custom"][CHILD_SESSIONS_EXTENSION]["maxOpen"],
        json!(1)
    );
    // a card without the offer has no key
    let none = agent_capabilities(&AgentId::new("chat"), "Chat", Some(&CardFacts::default()));
    assert!(
        serde_json::to_value(&none)
            .unwrap()
            .get("custom")
            .is_none_or(|c| c.get(CHILD_SESSIONS_EXTENSION).is_none())
    );
}

#[test]
fn a_card_that_lists_commands_declares_them_sorted_under_the_uri() {
    // the web draws its `/` menu from this key (ADR 0079); a hint is declared only when it has one
    let offer = CommandsOffer::from_params(&json!({
        "commands": [
            {"name": "review", "description": "Reviews a change.", "argumentHint": "<repo>"},
            {"name": "explain", "description": "Explains a file."},
        ],
    }))
    .unwrap();
    let card = CardFacts {
        commands: Some(offer),
        ..CardFacts::default()
    };
    let doc = agent_capabilities(&AgentId::new("coder"), "Coder", Some(&card));
    assert_capabilities_conform(&doc);
    assert_eq!(
        serde_json::to_value(&doc).unwrap()["custom"],
        json!({COMMANDS_EXTENSION: {"commands": [
            {"name": "explain", "description": "Explains a file."},
            {"name": "review", "description": "Reviews a change.", "argumentHint": "<repo>"}
        ]}}),
        "sorted by name, the hint beside the description"
    );
}

#[test]
fn a_card_that_lists_no_commands_or_an_unreadable_one_has_no_key_for_it() {
    // absent and malformed are both "no offer": the key is never written, so the goldens of the e2e
    // fake (whose card lists no command) do not move
    for card in [None, Some(CardFacts::default())] {
        let doc = agent_capabilities(&AgentId::new("x"), "X", card.as_ref());
        assert_capabilities_conform(&doc);
        let json = serde_json::to_value(&doc).unwrap();
        assert!(
            json.get("custom")
                .is_none_or(|c| c.get(COMMANDS_EXTENSION).is_none()),
            "{json}"
        );
    }
}
