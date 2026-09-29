//! `AgentCapabilities`: wire shape, round trip, and the schema as the oracle.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_agui_proto::testkit::{
    assert_capabilities_conform, assert_capabilities_json_conforms, capabilities_errors,
};
use orch_agui_proto::{
    AgentCapabilities, HumanInTheLoopCapabilities, IdentityCapabilities, MultiAgentCapabilities,
    SubagentInfo, TransportCapabilities,
};
use serde_json::json;

fn everything() -> AgentCapabilities {
    AgentCapabilities {
        identity: Some(IdentityCapabilities {
            name: Some("Coder".to_owned()),
            kind: Some("a2a".to_owned()),
            description: Some("Codes".to_owned()),
            version: Some("1.2.0".to_owned()),
            provider: Some("vymalo".to_owned()),
            documentation_url: Some("https://example.com/docs".to_owned()),
            metadata: Some(serde_json::Map::from_iter([("k".to_owned(), json!("v"))])),
        }),
        transport: Some(TransportCapabilities {
            streaming: Some(true),
            websocket: Some(false),
            http_binary: Some(false),
            push_notifications: Some(false),
            resumable: Some(true),
        }),
        multi_agent: Some(MultiAgentCapabilities {
            supported: Some(true),
            delegation: Some(true),
            handoffs: Some(false),
            subagents: Some(vec![SubagentInfo {
                name: "coder".to_owned(),
                description: Some("Codes".to_owned()),
            }]),
        }),
        human_in_the_loop: Some(HumanInTheLoopCapabilities {
            supported: Some(true),
            interrupts: Some(true),
        }),
        custom: Some([("https://example.com/x".to_owned(), json!({"any": [1, 2]}))].into()),
    }
}

#[test]
fn every_member_set_is_the_wire_json_and_conforms() {
    let doc = everything();
    assert_capabilities_conform(&doc);
    let json = serde_json::to_value(&doc).unwrap();
    assert_eq!(
        json,
        json!({
            "identity": {
                "name": "Coder", "type": "a2a", "description": "Codes", "version": "1.2.0",
                "provider": "vymalo", "documentationUrl": "https://example.com/docs",
                "metadata": {"k": "v"}
            },
            "transport": {
                "streaming": true, "websocket": false, "httpBinary": false,
                "pushNotifications": false, "resumable": true
            },
            "multiAgent": {
                "supported": true, "delegation": true, "handoffs": false,
                "subagents": [{"name": "coder", "description": "Codes"}]
            },
            "humanInTheLoop": {"supported": true, "interrupts": true},
            "custom": {"https://example.com/x": {"any": [1, 2]}}
        })
    );
    let back: AgentCapabilities = serde_json::from_value(json).unwrap();
    assert_eq!(back, doc);
}

#[test]
fn nothing_declared_is_an_empty_object_never_nulls() {
    let doc = AgentCapabilities::default();
    assert_capabilities_conform(&doc);
    assert_eq!(serde_json::to_value(&doc).unwrap(), json!({}));
    let partial = AgentCapabilities {
        transport: Some(TransportCapabilities {
            resumable: Some(true),
            ..TransportCapabilities::default()
        }),
        ..AgentCapabilities::default()
    };
    assert_eq!(
        serde_json::to_value(&partial).unwrap(),
        json!({"transport": {"resumable": true}})
    );
}

#[test]
fn the_schema_can_say_no() {
    // The oracle is closed: an undeclared member, or a subagent list that is a flag, fail.
    assert!(
        !capabilities_errors(&json!({"transport": {"streaming": true, "made_up": 1}})).is_empty()
    );
    assert!(!capabilities_errors(&json!({"multiAgent": {"subagents": true}})).is_empty());
    assert!(!capabilities_errors(&json!({"identity": {"name": 5}})).is_empty());
    assert_capabilities_json_conforms(&json!({"custom": {"anything": null}}));
}
